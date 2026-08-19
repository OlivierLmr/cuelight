//! SDR harness — deterministic discrete-event simulator for the distributed systems labs.
//!
//! The harness *is* the network. Student nodes are separate processes speaking JSON lines on
//! stdin/stdout; the harness owns logical time, routes every message, injects faults, and checks
//! properties. Runs are fully deterministic: the same seed replays exactly.

mod journal;
mod node;
mod proto;
mod rng;
mod sim;

use std::path::PathBuf;
use std::process::ExitCode;

const USAGE: &str = "\
sdr-harness — deterministic harness for the SDR labs

USAGE:
    sdr-harness run   --bin <cmd...> [options]
    sdr-harness check --bin <cmd...> [options]     run twice, verify identical journals

OPTIONS:
    --bin <cmd...>     command to launch one node (must be last; rest of line is the command)
    --nodes <n>        number of nodes            [default: 4]
    --faults <f>       crashes tolerated          [default: 1]
    --seed <s>         PRNG seed                  [default: 1]
    --min-delay <t>    min message delay          [default: 1]
    --max-delay <t>    max message delay          [default: 20]
    --time-limit <t>   logical time limit         [default: 10000]
    --out <dir>        run directory              [default: store/latest]
";

struct Args {
    program: Vec<String>,
    node_count: usize,
    f: usize,
    seed: u64,
    min_delay: u64,
    max_delay: u64,
    time_limit: u64,
    out: PathBuf,
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args {
        program: vec![],
        node_count: 4,
        f: 1,
        seed: 1,
        min_delay: 1,
        max_delay: 20,
        time_limit: 10_000,
        out: PathBuf::from("store/latest"),
    };
    let mut i = 0;
    while i < argv.len() {
        let need = |i: usize| -> Result<&String, String> {
            argv.get(i + 1).ok_or_else(|| format!("{} needs a value", argv[i]))
        };
        match argv[i].as_str() {
            // --bin swallows the remainder so node commands may carry their own flags.
            "--bin" => {
                a.program = argv[i + 1..].to_vec();
                if a.program.is_empty() {
                    return Err("--bin needs a command".into());
                }
                return Ok(a);
            }
            "--nodes" => { a.node_count = need(i)?.parse().map_err(|_| "bad --nodes")?; i += 2 }
            "--faults" => { a.f = need(i)?.parse().map_err(|_| "bad --faults")?; i += 2 }
            "--seed" => { a.seed = need(i)?.parse().map_err(|_| "bad --seed")?; i += 2 }
            "--min-delay" => { a.min_delay = need(i)?.parse().map_err(|_| "bad --min-delay")?; i += 2 }
            "--max-delay" => { a.max_delay = need(i)?.parse().map_err(|_| "bad --max-delay")?; i += 2 }
            "--time-limit" => { a.time_limit = need(i)?.parse().map_err(|_| "bad --time-limit")?; i += 2 }
            "--out" => { a.out = PathBuf::from(need(i)?); i += 2 }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Err("missing --bin".into())
}

fn config(a: &Args, out: PathBuf) -> sim::Config {
    sim::Config {
        program: a.program.clone(),
        node_count: a.node_count,
        f: a.f,
        seed: a.seed,
        min_delay: a.min_delay,
        max_delay: a.max_delay,
        time_limit: a.time_limit,
        run_dir: out,
    }
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = argv.first().cloned() else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };

    let a = match parse(&argv[1..]) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match cmd.as_str() {
        "run" => match sim::Sim::new(config(&a, a.out.clone())).and_then(|s| s.run()) {
            Ok(()) => {
                println!("ok — journal at {}", a.out.join("journal.jsonl").display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("error: {e}");
                ExitCode::FAILURE
            }
        },
        // The M0 acceptance test: identical seed must produce a byte-identical journal.
        "check" => {
            let mut hashes = vec![];
            for pass in 0..2 {
                let dir = a.out.join(format!("check-{pass}"));
                if let Err(e) = sim::Sim::new(config(&a, dir.clone())).and_then(|s| s.run()) {
                    eprintln!("error: {e}");
                    return ExitCode::FAILURE;
                }
                match std::fs::read(dir.join("journal.jsonl")) {
                    Ok(b) => hashes.push(b),
                    Err(e) => {
                        eprintln!("error reading journal: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            if hashes[0] == hashes[1] {
                println!(
                    "DETERMINISTIC — seed {} produced byte-identical journals ({} bytes)",
                    a.seed,
                    hashes[0].len()
                );
                ExitCode::SUCCESS
            } else {
                eprintln!("NONDETERMINISTIC — journals differ for seed {}", a.seed);
                eprintln!("  compare {}/check-0 and {}/check-1", a.out.display(), a.out.display());
                ExitCode::FAILURE
            }
        }
        other => {
            eprintln!("unknown command {other}\n\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}
