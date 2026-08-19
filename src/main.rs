//! SDR harness — deterministic discrete-event simulator for the distributed systems labs.
//!
//! The harness *is* the network. Student nodes are separate processes speaking JSON lines on
//! stdin/stdout; the harness owns logical time, routes every message, injects faults and checks
//! properties. Runs replay exactly from their scenario.

mod journal;
mod node;
mod proto;
mod rng;
mod scenario;
mod sim;
mod viz;

use scenario::{ExpandOpts, Scenario, Workload};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "\
sdr-harness — deterministic harness for the SDR labs

USAGE:
    sdr-harness run    [options] --bin <cmd...>    one run
    sdr-harness check  [options] --bin <cmd...>    run twice, verify identical journals
    sdr-harness sweep  [options] --bin <cmd...>    many seeds, report pass/fail
    sdr-harness viz    --journal <path> [--out <path>]

OPTIONS:
    --bin <cmd...>     command launching one node (MUST BE LAST: swallows the rest of the line)
    --scenario <path>  replay a stored scenario instead of expanding a seed
    --seed <s>         seed to expand                [default: 1]
    --seeds <n>        sweep: how many seeds         [default: 100]
    --nodes <n>        node count                    [default: 4]
    --faults <f>       crashes tolerated             [default: 1]
    --no-faults        expand a clean run
    --fifo             per-link FIFO ordering (Lamport's mutex needs it)
    --workload <w>     none|broadcast|mutex|consensus  [default: none]
    --time-limit <t>   logical time limit            [default: 10000]
    --watchdog <ms>    wall-clock hang detector      [default: 5000]
    --out <dir>        run directory                 [default: store/latest]
    --journal <path>   viz: journal to render
";

struct Args {
    program: Vec<String>,
    scenario_path: Option<PathBuf>,
    journal: Option<PathBuf>,
    seed: u64,
    seeds: u64,
    nodes: usize,
    f: usize,
    with_faults: bool,
    fifo: bool,
    workload: Workload,
    time_limit: u64,
    watchdog: u64,
    out: PathBuf,
}

impl Default for Args {
    fn default() -> Self {
        Args {
            program: vec![],
            scenario_path: None,
            journal: None,
            seed: 1,
            seeds: 100,
            nodes: 4,
            f: 1,
            with_faults: true,
            fifo: false,
            workload: Workload::None,
            time_limit: 10_000,
            watchdog: 5_000,
            out: PathBuf::from("store/latest"),
        }
    }
}

fn parse(argv: &[String]) -> Result<Args, String> {
    let mut a = Args::default();
    let mut i = 0;
    while i < argv.len() {
        let val = |i: usize| -> Result<String, String> {
            argv.get(i + 1).cloned().ok_or_else(|| format!("{} needs a value", argv[i]))
        };
        match argv[i].as_str() {
            "--bin" => {
                a.program = argv[i + 1..].to_vec();
                if a.program.is_empty() {
                    return Err("--bin needs a command".into());
                }
                return Ok(a);
            }
            "--scenario" => { a.scenario_path = Some(PathBuf::from(val(i)?)); i += 2 }
            "--journal" => { a.journal = Some(PathBuf::from(val(i)?)); i += 2 }
            "--seed" => { a.seed = val(i)?.parse().map_err(|_| "bad --seed")?; i += 2 }
            "--seeds" => { a.seeds = val(i)?.parse().map_err(|_| "bad --seeds")?; i += 2 }
            "--nodes" => { a.nodes = val(i)?.parse().map_err(|_| "bad --nodes")?; i += 2 }
            "--faults" => { a.f = val(i)?.parse().map_err(|_| "bad --faults")?; i += 2 }
            "--time-limit" => { a.time_limit = val(i)?.parse().map_err(|_| "bad --time-limit")?; i += 2 }
            "--watchdog" => { a.watchdog = val(i)?.parse().map_err(|_| "bad --watchdog")?; i += 2 }
            "--out" => { a.out = PathBuf::from(val(i)?); i += 2 }
            "--workload" => { a.workload = Workload::parse(&val(i)?)?; i += 2 }
            "--no-faults" => { a.with_faults = false; i += 1 }
            "--fifo" => { a.fifo = true; i += 1 }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(a)
}

fn scenario_for(a: &Args, seed: u64) -> Result<Scenario, String> {
    match &a.scenario_path {
        Some(p) => Scenario::from_json(
            &std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?,
        ),
        None => Ok(Scenario::expand(
            seed,
            &ExpandOpts {
                nodes: a.nodes,
                f: a.f,
                time_limit: a.time_limit,
                fifo: a.fifo,
                with_faults: a.with_faults,
                workload: a.workload,
            },
        )),
    }
}

fn config(a: &Args, sc: Scenario, out: PathBuf) -> sim::Config {
    sim::Config {
        program: a.program.clone(),
        scenario: sc,
        run_dir: out,
        watchdog: Duration::from_millis(a.watchdog),
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
        Err(e) => { eprintln!("error: {e}\n\n{USAGE}"); return ExitCode::FAILURE }
    };
    if cmd != "viz" && a.program.is_empty() {
        eprintln!("error: missing --bin\n\n{USAGE}");
        return ExitCode::FAILURE;
    }

    match cmd.as_str() {
        "run" => {
            let sc = match scenario_for(&a, a.seed) {
                Ok(s) => s,
                Err(e) => { eprintln!("error: {e}"); return ExitCode::FAILURE }
            };
            match sim::Sim::new(config(&a, sc, a.out.clone())).and_then(|s| s.run()) {
                Ok(o) => {
                    println!(
                        "ok — {} at t={} ({} events)\n     {}",
                        o.reason, o.end_time, o.events, a.out.display()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => { eprintln!("FAILED: {e}"); ExitCode::FAILURE }
            }
        }

        // Acceptance test: same scenario must produce a byte-identical journal.
        "check" => {
            let mut js = vec![];
            for pass in 0..2 {
                let sc = match scenario_for(&a, a.seed) {
                    Ok(s) => s,
                    Err(e) => { eprintln!("error: {e}"); return ExitCode::FAILURE }
                };
                let dir = a.out.join(format!("check-{pass}"));
                if let Err(e) = sim::Sim::new(config(&a, sc, dir.clone())).and_then(|s| s.run()) {
                    eprintln!("FAILED: {e}");
                    return ExitCode::FAILURE;
                }
                match std::fs::read(dir.join("journal.jsonl")) {
                    Ok(b) => js.push(b),
                    Err(e) => { eprintln!("error reading journal: {e}"); return ExitCode::FAILURE }
                }
            }
            if js[0] == js[1] {
                println!("DETERMINISTIC — seed {} byte-identical ({} bytes)", a.seed, js[0].len());
                ExitCode::SUCCESS
            } else {
                eprintln!("NONDETERMINISTIC — journals differ for seed {}", a.seed);
                eprintln!("  compare {0}/check-0 and {0}/check-1", a.out.display());
                ExitCode::FAILURE
            }
        }

        "sweep" => {
            let mut pass = 0u64;
            let mut fails: Vec<(u64, String)> = vec![];
            for seed in 1..=a.seeds {
                let sc = match scenario_for(&a, seed) {
                    Ok(s) => s,
                    Err(e) => { eprintln!("error: {e}"); return ExitCode::FAILURE }
                };
                let dir = a.out.join("sweep").join(seed.to_string());
                match sim::Sim::new(config(&a, sc, dir.clone())).and_then(|s| s.run()) {
                    Ok(_) => {
                        pass += 1;
                        let _ = std::fs::remove_dir_all(&dir); // keep only failures
                    }
                    Err(e) => fails.push((seed, e)),
                }
            }
            println!("{pass}/{} seeds passed", a.seeds);
            for (s, e) in fails.iter().take(10) {
                println!("  seed {s}: {e}");
                println!("    replay: --scenario {}/sweep/{s}/scenario.json", a.out.display());
            }
            if fails.len() > 10 {
                println!("  ... and {} more", fails.len() - 10);
            }
            if fails.is_empty() { ExitCode::SUCCESS } else { ExitCode::FAILURE }
        }

        "viz" => {
            let Some(j) = a.journal.clone() else {
                eprintln!("error: viz needs --journal");
                return ExitCode::FAILURE;
            };
            let out = a.out.join("messages.mmd");
            if let Some(p) = out.parent() { let _ = std::fs::create_dir_all(p); }
            match viz::render(&j, &out, 200) {
                Ok(n) => { println!("wrote {} ({n} messages)", out.display()); ExitCode::SUCCESS }
                Err(e) => { eprintln!("error: {e}"); ExitCode::FAILURE }
            }
        }

        other => { eprintln!("unknown command {other}\n\n{USAGE}"); ExitCode::FAILURE }
    }
}
