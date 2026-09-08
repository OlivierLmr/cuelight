//! Drive cuelight over a suite of scenarios, judge the journals it writes.
//!
//! cuelight executes one scenario and records what happened. It has no notion of success: it does
//! not know what a property is, or what a test is. Everything on this side of that line lives here:
//! running campaigns, loading journals, dating liveness from the effective GST, and printing a
//! verdict.
//!
//! A caller supplies a [`Suite`]: its campaigns, its directed scenarios with the failures it
//! *expects*, and one function that turns a run into a [`Report`]. This crate never inspects that
//! function; it calls it.
//!
//! ```no_run
//! use cuelight_suite::{Campaign, Events, Kind, Report, Scenario, Suite};
//! use std::process::ExitCode;
//!
//! static CAMPAIGNS: &[Campaign] = &[Campaign {
//!     label: "steady", stimuli: "stimuli/steady.json", fifo: true, faults: true, seeds: 200,
//! }];
//!
//! fn check(ev: &Events, _sc: &Scenario, r: &mut Report) {
//!     r.add("said-something", Kind::Safety, !ev.observes.is_empty(), "…".into());
//! }
//!
//! fn main() -> ExitCode {
//!     cuelight_suite::run(
//!         Suite { name: "mine", campaigns: CAMPAIGNS, directed: &[], check },
//!         env!("CARGO_MANIFEST_DIR"),
//!     )
//! }
//! ```

use cuelight::scenario::{ExpandOpts, Fault, StimulusSpec};
use cuelight::sim;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

/// The scenario that was run, as cuelight expanded or loaded it.
pub use cuelight::Scenario;

// ---------------------------------------------------------------- properties

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Safety,
    Liveness,
}

#[derive(Debug)]
pub struct Check {
    pub name: &'static str,
    pub kind: Kind,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    pub fn add(&mut self, name: &'static str, kind: Kind, ok: bool, detail: String) {
        self.checks.push(Check { name, kind, ok, detail });
    }
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
    /// Names of the checks that failed, in declaration order.
    pub fn failed(&self) -> Vec<&'static str> {
        self.checks.iter().filter(|c| !c.ok).map(|c| c.name).collect()
    }
    pub fn print(&self) {
        for c in &self.checks {
            let k = if c.kind == Kind::Safety { "safety  " } else { "liveness" };
            println!("  {} {k} {:<20} {}", if c.ok { "ok  " } else { "FAIL" }, c.name, c.detail);
        }
    }
}

// ------------------------------------------------------------- what a run is

/// Everything a property can be stated over: what nodes observed, what the harness asked of them,
/// and who died.
pub struct Events {
    /// `(t, node, body)`: things a node reported to the harness.
    pub observes: Vec<(u64, String, Value)>,
    /// `(t, node, body)`: things the harness asked of a node.
    pub stimuli: Vec<(u64, String, Value)>,
    pub crashed: HashMap<String, u64>,
    pub end: u64,
}

/// Where liveness deadlines date from, **never** [`Scenario::gst`].
///
/// A pause landing after GST also violates partial synchrony, so the effective GST is the end of
/// the last fault. Dating from `gst` would fail correct implementations.
pub fn effective_gst(sc: &Scenario) -> u64 {
    sc.faults.iter().fold(sc.gst, |t, f| {
        t.max(match f {
            Fault::Crash { at, .. } => *at,
            Fault::Pause { at, duration, .. } | Fault::Partition { at, duration, .. } => {
                at + duration
            }
        })
    })
}

/// Nodes the run never crashed. Properties are stated over these, not over all nodes.
pub fn correct_nodes(sc: &Scenario, ev: &Events) -> Vec<String> {
    (0..sc.nodes).map(|i| format!("n{i}")).filter(|n| !ev.crashed.contains_key(n)).collect()
}

pub fn ty(b: &Value) -> &str {
    b.get("type").and_then(Value::as_str).unwrap_or("")
}

/// Read back the journal cuelight wrote. The scenario is not read back: the caller ran it, so it
/// already holds the very object the run was driven from.
pub fn load_events(dir: &Path) -> Result<Events, String> {
    let jpath = dir.join("journal.jsonl");
    let f = std::fs::File::open(&jpath).map_err(|e| format!("{}: {e}", jpath.display()))?;
    let mut ev = Events { observes: vec![], stimuli: vec![], crashed: HashMap::new(), end: 0 };
    for line in BufReader::new(f).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let t = v.get("t").and_then(Value::as_u64).unwrap_or(0);
        ev.end = ev.end.max(t);
        let body = v.get("body").cloned().unwrap_or(Value::Null);
        match v.get("kind").and_then(Value::as_str).unwrap_or("") {
            "observe" => {
                let src = v.get("src").and_then(Value::as_str).unwrap_or("").to_string();
                ev.observes.push((t, src, body));
            }
            "stimulus" => {
                let dst = v.get("dest").and_then(Value::as_str).unwrap_or("").to_string();
                ev.stimuli.push((t, dst, body));
            }
            "fault-crash" => {
                if let Some(n) = v.pointer("/detail/node").and_then(Value::as_str) {
                    ev.crashed.insert(n.to_string(), t);
                }
            }
            _ => {}
        }
    }
    Ok(ev)
}

// ------------------------------------------------------------------- a suite

/// A seed campaign: the same stimulus template expanded over many seeds.
///
/// One run proves nothing. The bugs a course like this is about show up on a minority of seeds: a
/// student running once has a 90% chance of concluding a broken mutex works.
pub struct Campaign {
    pub label: &'static str,
    /// Stimulus template, relative to the suite directory. Empty means the workload is the faults
    /// alone, which is what a failure detector driven by crashes and time needs.
    pub stimuli: &'static str,
    pub fifo: bool,
    /// False expands a clean run, the right setting for an algorithm that assumes no failures.
    pub faults: bool,
    pub seeds: u64,
}

/// A stored scenario, run once, with the failures a *correct* implementation should produce.
///
/// `expect_fail` is not a formality. A scenario can exist to show what an algorithm's assumptions
/// cost when they do not hold, and a suite that could only say "all green" could not express one.
pub struct Directed {
    /// Scenario path, relative to the suite directory.
    pub path: &'static str,
    pub expect_fail: &'static [&'static str],
    pub why: &'static str,
}

pub struct Suite {
    pub name: &'static str,
    pub campaigns: &'static [Campaign],
    pub directed: &'static [Directed],
    pub check: fn(&Events, &Scenario, &mut Report),
}

// ------------------------------------------------------------------- running

struct Opts {
    suite_dir: PathBuf,
    out: PathBuf,
    seeds: Option<u64>,
    /// Inclusive seed range. Debugging one failure should not mean re-running the other 199.
    range: Option<(u64, u64)>,
    /// Substring selecting which campaigns and directed scenarios to run.
    only: Option<String>,
    list: bool,
    watchdog: u64,
    program: Vec<String>,
}

/// The name a directed scenario is selected by: its file stem.
fn stem(path: &str) -> &str {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("directed")
}

fn usage(name: &str) -> String {
    format!(
        "\
{name}: run this suite's scenarios and judge the journals they write

USAGE:
    {name} [options] --bin <cmd...>

OPTIONS:
    --bin <cmd...>     command launching one node (MUST BE LAST: swallows the rest of the line)
    --suite-dir <path> where this suite's scenarios/ and stimuli/ live
    --out <dir>        run directory                 [default: store/<suite>]
    --seeds <n>        override every campaign's seed count
    --seed <a>[..<b>]  run one seed, or an inclusive range, instead of a whole campaign
    --only <name>      run only what matches: a campaign label or a scenario name
    --watchdog <ms>    wall-clock hang detector      [default: 5000]
    --list             show the campaigns and scenarios this suite defines, then exit
"
    )
}

fn parse(argv: &[String], suite: &str, default_suite_dir: &str) -> Result<Opts, String> {
    let mut o = Opts {
        suite_dir: PathBuf::from(default_suite_dir),
        out: PathBuf::from("store").join(suite),
        seeds: None,
        range: None,
        only: None,
        list: false,
        watchdog: 5_000,
        program: vec![],
    };
    let mut i = 0;
    while i < argv.len() {
        let val = |i: usize| -> Result<String, String> {
            argv.get(i + 1).cloned().ok_or_else(|| format!("{} needs a value", argv[i]))
        };
        match argv[i].as_str() {
            "--bin" => {
                o.program = argv[i + 1..].to_vec();
                if o.program.is_empty() {
                    return Err("--bin needs a command".into());
                }
                return Ok(o);
            }
            "--suite-dir" => { o.suite_dir = PathBuf::from(val(i)?); i += 2 }
            "--out" => { o.out = PathBuf::from(val(i)?); i += 2 }
            "--seeds" => { o.seeds = Some(val(i)?.parse().map_err(|_| "bad --seeds")?); i += 2 }
            "--watchdog" => { o.watchdog = val(i)?.parse().map_err(|_| "bad --watchdog")?; i += 2 }
            "--seed" => {
                let v = val(i)?;
                let (a, b) = match v.split_once("..") {
                    Some((a, b)) => (a, b),
                    None => (v.as_str(), v.as_str()),
                };
                let lo: u64 = a.trim().parse().map_err(|_| format!("bad --seed {v}"))?;
                let hi: u64 = b.trim().parse().map_err(|_| format!("bad --seed {v}"))?;
                if hi < lo {
                    return Err(format!("--seed {v}: the range runs backwards"));
                }
                o.range = Some((lo, hi));
                i += 2
            }
            "--only" => { o.only = Some(val(i)?); i += 2 }
            "--list" => { o.list = true; i += 1 }
            other => return Err(format!("unknown option {other}")),
        }
    }
    Ok(o)
}

/// How a campaign's seeds expand: its own switches, and its stimulus template read from disk.
fn expand_opts(o: &Opts, c: &Campaign) -> Result<ExpandOpts, String> {
    let stimuli = if c.stimuli.is_empty() {
        None
    } else {
        let p = o.suite_dir.join(c.stimuli);
        let raw = std::fs::read_to_string(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        Some(StimulusSpec::from_json(&raw)?)
    };
    Ok(ExpandOpts { fifo: c.fifo, with_faults: c.faults, stimuli, ..ExpandOpts::default() })
}

fn load_scenario(path: &Path) -> Result<Scenario, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Scenario::from_json(&raw)
}

/// Render the sequence diagram beside the journal.
///
/// Only for runs worth opening: a campaign deletes the seeds that pass, and rendering hundreds of
/// diagrams nobody looks at would be waste.
fn draw(dir: &Path) -> Option<PathBuf> {
    let out = dir.join("messages.mmd");
    cuelight::viz::render(&dir.join("journal.jsonl"), &out, 200).ok().map(|_| out)
}

/// Where to read what happened. The run directory holds it all, but nobody guesses its path.
fn where_to_read(dir: &Path) {
    let journal = dir.join("journal.jsonl");
    if journal.exists() {
        println!("    journal: {}", journal.display());
    }
    if let Some(mmd) = draw(dir) {
        println!("    diagram: {}", mmd.display());
    }
}

/// The command that runs this one case again, ready to paste.
fn how_to_replay(name: &str, o: &Opts, selector: &str) {
    println!("    replay:  {name} {selector} --bin {}", o.program.join(" "));
}

/// One simulation. Returns what the run wrote, or why it could not run.
fn run_once(o: &Opts, dir: &Path, scenario: Scenario) -> Result<(), String> {
    sim::Sim::new(sim::Config {
        program: o.program.clone(),
        scenario,
        run_dir: dir.to_path_buf(),
        watchdog: Duration::from_millis(o.watchdog),
    })?
    .run()
    .map(|_| ())
}

/// Replay one case and compare the two journals, before judging anything.
///
/// A node that reads the system clock, spawns a thread or draws unseeded randomness produces a
/// different run every time. Every verdict below would then describe a run nobody can reproduce,
/// and the replay command printed under each failure would replay something else. So this runs
/// first, and stops the suite when it fails, rather than appearing as one line among the results.
///
/// Two runs, against several hundred: the cost is invisible.
fn replays_identically(o: &Opts, suite: &Suite) -> Result<String, String> {
    let (what, sc) = match suite.directed.first() {
        Some(d) => (stem(d.path).to_string(), load_scenario(&o.suite_dir.join(d.path))?),
        None => match suite.campaigns.first() {
            Some(c) => (format!("{} seed 1", c.label), Scenario::expand(1, &expand_opts(o, c)?)),
            None => return Ok(String::new()),
        },
    };

    let mut runs = vec![];
    for pass in ["a", "b"] {
        let dir = o.out.join("replay").join(pass);
        run_once(o, &dir, sc.clone())?;
        let j = dir.join("journal.jsonl");
        let text = std::fs::read_to_string(&j).map_err(|e| format!("{}: {e}", j.display()))?;
        runs.push((j, text));
    }

    if runs[0].1 == runs[1].1 {
        let _ = std::fs::remove_dir_all(o.out.join("replay"));
        return Ok(what);
    }

    // Name the line, because "the journals differ" sends someone to diff two thousand-line files.
    let line = runs[0]
        .1
        .lines()
        .zip(runs[1].1.lines())
        .position(|(a, b)| a != b)
        .map(|i| i + 1)
        .unwrap_or_else(|| runs[0].1.lines().count().min(runs[1].1.lines().count()) + 1);

    Err(format!(
        "replaying {what} does not give the same journal twice.\n\
         \x20 First difference on line {line}. A system clock, a thread, or unseeded randomness.\n\
         \x20 compare {}\n\
         \x20     and {}",
        runs[0].0.display(),
        runs[1].0.display()
    ))
}

/// Entry point for the binary that owns a suite:
/// `fn main() -> ExitCode { cuelight_suite::run(SUITE, env!("CARGO_MANIFEST_DIR")) }`
pub fn run(suite: Suite, default_suite_dir: &str) -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let name = format!("{}-check", suite.name);
    let o = match parse(&argv, suite.name, default_suite_dir) {
        Ok(o) => o,
        Err(e) => { eprintln!("error: {e}\n\n{}", usage(&name)); return ExitCode::FAILURE }
    };
    if o.list {
        // Pad to the longest name this suite actually declares. A fixed width fits one suite and
        // runs the description into the name of every other.
        let w = suite
            .campaigns
            .iter()
            .map(|c| c.label.len())
            .chain(suite.directed.iter().map(|d| stem(d.path).len()))
            .max()
            .unwrap_or(0)
            .max(8); // a floor, so a suite with one short name still reads as a column
        println!("campaigns:");
        for c in suite.campaigns {
            println!("  {:<w$} {} seeds, fifo={}, faults={}", c.label, c.seeds, c.fifo, c.faults);
        }
        println!("directed scenarios:");
        for d in suite.directed {
            println!("  {:<w$} {}", stem(d.path), d.why);
        }
        return ExitCode::SUCCESS;
    }

    if o.program.is_empty() {
        eprintln!("error: missing --bin\n\n{}", usage(&name));
        return ExitCode::FAILURE;
    }

    // Nothing matched is a mistake, not an empty run: silently doing nothing looks like success.
    if let Some(pat) = &o.only {
        let known = suite.campaigns.iter().any(|c| c.label.contains(pat.as_str()))
            || suite.directed.iter().any(|d| stem(d.path).contains(pat.as_str()));
        if !known {
            eprintln!("--only {pat} matches nothing. `--list` shows what this suite defines.");
            return ExitCode::FAILURE;
        }
    }

    // First, and on its own: nothing below means anything if the node is not deterministic.
    match replays_identically(&o, &suite) {
        Ok(what) if what.is_empty() => {}
        Ok(what) => println!("deterministic: replaying {what} gives the same journal twice\n"),
        Err(e) => { eprintln!("NOT DETERMINISTIC: {e}"); return ExitCode::FAILURE }
    }

    let mut all_ok = true;

    for c in suite.campaigns {
        if let Some(pat) = &o.only {
            if !c.label.contains(pat.as_str()) {
                continue;
            }
        }
        // Read once per campaign rather than once per seed: it is the same template every time.
        let opts = match expand_opts(&o, c) {
            Ok(x) => x,
            Err(e) => { println!("{}: {e}", c.label); all_ok = false; continue }
        };

        let seeds = o.seeds.unwrap_or(c.seeds);
        let (lo, hi) = o.range.unwrap_or((1, seeds));
        let mut pass = 0u64;
        let mut fails: Vec<(u64, String)> = vec![];
        for seed in lo..=hi {
            let dir = o.out.join(c.label).join(seed.to_string());
            let sc = Scenario::expand(seed, &opts);
            match run_once(&o, &dir, sc.clone()) {
                Err(e) => fails.push((seed, e)),
                Ok(()) => match load_events(&dir) {
                    Err(e) => fails.push((seed, e)),
                    Ok(ev) => {
                        let mut r = Report::default();
                        (suite.check)(&ev, &sc, &mut r);
                        if r.ok() {
                            pass += 1;
                            let _ = std::fs::remove_dir_all(&dir); // keep only failures
                        } else {
                            fails.push((seed, r.failed().join(", ")));
                        }
                    }
                },
            }
        }
        println!("{}: {pass}/{} seeds passed{}", c.label, hi - lo + 1,
                 if o.range.is_some() { format!(" (seeds {lo}..{hi})") } else { String::new() });
        for (s, e) in fails.iter().take(5) {
            println!("  seed {s}: {e}");
            how_to_replay(&name, &o, &format!("--seed {s} --only {}", c.label));
            where_to_read(&o.out.join(c.label).join(s.to_string()));
        }
        if fails.len() > 5 {
            println!("  ... and {} more", fails.len() - 5);
        }
        all_ok &= fails.is_empty();
    }

    for d in suite.directed {
        // A directed test has no seed, so asking for one means asking for campaigns only.
        if o.range.is_some() {
            continue;
        }
        if let Some(pat) = &o.only {
            if !stem(d.path).contains(pat.as_str()) {
                continue;
            }
        }
        let path = o.suite_dir.join(d.path);
        let stem = stem(d.path);
        let dir = o.out.join("directed").join(stem);
        println!("\n{}: {}", d.path, d.why);
        let sc = match load_scenario(&path) {
            Ok(sc) => sc,
            Err(e) => { println!("  did not run: {e}"); all_ok = false; continue }
        };
        match run_once(&o, &dir, sc.clone()) {
            Err(e) => { println!("  did not run: {e}"); all_ok = false }
            Ok(()) => match load_events(&dir) {
                Err(e) => { println!("  {e}"); all_ok = false }
                Ok(ev) => {
                    let mut r = Report::default();
                    (suite.check)(&ev, &sc, &mut r);
                    r.print();
                    let got = r.failed();
                    // Expected-failure is the point of some directed scenarios: a suite that could
                    // only say "all green" could not express them.
                    if got == d.expect_fail {
                        println!("  → as expected");
                    } else {
                        println!("  → EXPECTED {:?}, GOT {:?}", d.expect_fail, got);
                        how_to_replay(&name, &o, &format!("--only {stem}"));
                        all_ok = false;
                    }
                    // Always, even when it behaved: a directed scenario is often run precisely to
                    // be looked at, and the diagram is the reason to look.
                    where_to_read(&dir);
                }
            },
        }
    }

    if all_ok {
        println!("\nall good");
        ExitCode::SUCCESS
    } else {
        println!("\nsomething failed, see above");
        ExitCode::FAILURE
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(json: &str) -> Scenario {
        Scenario::from_json(json).expect("scenario")
    }

    #[test]
    fn effective_gst_is_the_scheduled_one_when_nothing_disturbs_it() {
        assert_eq!(effective_gst(&sc(r#"{"gst": 500}"#)), 500);
    }

    #[test]
    fn a_crash_moves_it_to_the_crash() {
        let s = sc(r#"{"gst": 500, "faults": [{"kind": "crash", "at": 900, "node": "n1"}]}"#);
        assert_eq!(effective_gst(&s), 900);
    }

    #[test]
    fn a_pause_moves_it_to_the_end_of_the_pause_not_its_start() {
        let s = sc(r#"{"gst": 500, "faults":
            [{"kind": "pause", "at": 600, "node": "n1", "duration": 700}]}"#);
        assert_eq!(effective_gst(&s), 1300);
    }

    #[test]
    fn a_partition_counts_like_a_pause() {
        let s = sc(r#"{"gst": 500, "faults":
            [{"kind": "partition", "at": 600, "duration": 700, "side": ["n0"]}]}"#);
        assert_eq!(effective_gst(&s), 1300);
    }

    #[test]
    fn a_fault_that_ends_before_gst_leaves_it_alone() {
        let s = sc(r#"{"gst": 5000, "faults":
            [{"kind": "pause", "at": 100, "node": "n1", "duration": 200}]}"#);
        assert_eq!(effective_gst(&s), 5000);
    }

    #[test]
    fn the_latest_fault_wins_whatever_the_order() {
        let s = sc(r#"{"gst": 100, "faults": [
            {"kind": "pause", "at": 3000, "node": "n1", "duration": 10},
            {"kind": "crash", "at": 200, "node": "n2"}]}"#);
        assert_eq!(effective_gst(&s), 3010);
    }
}
