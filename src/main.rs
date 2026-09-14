//! The `cuelight` command line: argument parsing and printing, over the library of the same name.
//!
//! Every behaviour lives in the library, so that a checker calling `cuelight` as a dependency and a
//! person typing `cuelight run` exercise the same code.

use cuelight::scenario::{fingerprint, Drawn, EnvironmentSpace, Scenario, WorkloadSpace};
use cuelight::{sim, viz};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

const USAGE: &str = "\
cuelight: a deterministic discrete-event simulator for distributed systems

USAGE:
    cuelight run    [options] --bin <cmd...>    one run
    cuelight check  [options] --bin <cmd...>    run twice, verify identical journals
    cuelight viz    --journal <path> [--out <path>]
    cuelight scenario [options]                 print a drawn scenario, to edit by hand

The harness executes one scenario and records what happened; it never judges. Loop over seeds
from your own checker and point it at the journal each run writes (see JOURNAL.md).

OPTIONS:
    --bin <cmd...>     command launching one node (MUST BE LAST: swallows the rest of the line)
    --scenario <path>  replay a stored scenario instead of drawing one
    --seed <s>         seed to draw from              [default: 1]
    --environment <p>  what the run undergoes         [default: every field pinned, no faults]
    --workload <p>     what it is asked to do         [default: none]
    --watchdog <ms>    wall-clock hang detector       [default: 5000]
    --out <dir>        run directory                 [default: store/latest]
    --journal <path>   viz: journal to render
";

struct Args {
    program: Vec<String>,
    scenario_path: Option<PathBuf>,
    journal: Option<PathBuf>,
    seed: u64,
    env: EnvironmentSpace,
    /// Kept beside the parsed spaces so a drawn scenario can record where it came from, and what
    /// the files said at the time.
    env_name: String,
    env_src: String,
    work: Option<WorkloadSpace>,
    work_name: Option<String>,
    work_src: Option<String>,
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
            env: EnvironmentSpace::from_json("{}").expect("empty object is every default"),
            env_name: "(defaults)".into(),
            env_src: "{}".into(),
            work: None,
            work_name: None,
            work_src: None,
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
            "--watchdog" => { a.watchdog = val(i)?.parse().map_err(|_| "bad --watchdog")?; i += 2 }
            "--out" => { a.out = PathBuf::from(val(i)?); i += 2 }
            "--environment" => {
                let p = val(i)?;
                let raw = std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?;
                a.env = EnvironmentSpace::from_json(&raw)?;
                a.env_name = p;
                a.env_src = raw;
                i += 2
            }
            "--workload" => {
                let p = val(i)?;
                let raw = std::fs::read_to_string(&p).map_err(|e| format!("{p}: {e}"))?;
                a.work = Some(WorkloadSpace::from_json(&raw)?);
                a.work_name = Some(p);
                a.work_src = Some(raw);
                i += 2
            }
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
        None => Ok(Scenario::draw(seed, &a.env, a.work.as_ref()).from(Drawn {
            seed,
            environment: a.env_name.clone(),
            workload: a.work_name.clone(),
            fingerprint: fingerprint(&a.env_src, a.work_src.as_deref()),
        })),
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
    if !matches!(cmd.as_str(), "viz" | "scenario") && a.program.is_empty() {
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
                        "ok: {} at t={} ({} events)\n     {}",
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
                println!("DETERMINISTIC: seed {} byte-identical ({} bytes)", a.seed, js[0].len());
                ExitCode::SUCCESS
            } else {
                eprintln!("NONDETERMINISTIC: journals differ for seed {}", a.seed);
                eprintln!("  compare {0}/check-0 and {0}/check-1", a.out.display());
                ExitCode::FAILURE
            }
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

        // Dump-to-edit: the starting point for a scenario written by hand.
        "scenario" => match scenario_for(&a, a.seed) {
            Ok(sc) => { println!("{}", sc.to_json()); ExitCode::SUCCESS }
            Err(e) => { eprintln!("error: {e}"); ExitCode::FAILURE }
        },

        other => { eprintln!("unknown command {other}\n\n{USAGE}"); ExitCode::FAILURE }
    }
}
