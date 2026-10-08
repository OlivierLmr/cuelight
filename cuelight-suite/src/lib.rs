//! Drive cuelight over a suite of scenarios, judge the journals it writes.
//!
//! cuelight executes one scenario and records what happened. It has no notion of success: it does
//! not know what a property is, or what a test is. Everything on this side of that line lives here:
//! drawing scenarios over seeds, loading journals, dating liveness from the effective GST, and printing a
//! verdict.
//!
//! A caller supplies a [`Suite`]: its parametric, its written scenarios with the failures it
//! *expects*, and one function that turns a run into a [`Report`]. This crate never inspects that
//! function; it calls it.
//!
//! ```no_run
//! use cuelight_suite::{Events, Kind, Parametric, Report, Scenario, Suite};
//! use std::process::ExitCode;
//!
//! static SPACES: &[Parametric] =
//!     &[Parametric { space: "spaces/steady.json", seeds: 200 }];
//!
//! fn check(ev: &Events, _sc: &Scenario, r: &mut Report) {
//!     r.add("said-something", Kind::Safety, !ev.observes.is_empty(), "…".into());
//! }
//!
//! fn main() -> ExitCode {
//!     cuelight_suite::run(
//!         Suite { name: "mine", parametric: SPACES, written: &[], check,
//!                 style: Some("cuesheet.cuestyle") },
//!         env!("CARGO_MANIFEST_DIR"),
//!     )
//! }
//! ```

use cuelight::scenario::{fingerprint, Drawn, Fault, Space};
use cuelight::sim;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

/// The scenario that was run, as cuelight drew or loaded it.
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

/// One message a node sent to another node, as the journal's `send` entry records it.
#[derive(Debug, Clone, PartialEq)]
pub struct Sent {
    /// When it left, in logical time. Not when it arrived, if it ever did.
    pub t: u64,
    pub src: String,
    pub dest: String,
    /// Which message this is: the journal's `mid`, dense from 1 in the order messages leave.
    pub mid: u64,
    pub body: Value,
}

/// Everything a property can be stated over: what nodes observed, what the harness asked of them,
/// and who died.
pub struct Events {
    /// `(t, node, body)`: things a node reported to the harness.
    pub observes: Vec<(u64, String, Value)>,
    /// `(t, node, body)`: things the harness asked of a node.
    pub stimuli: Vec<(u64, String, Value)>,
    /// Every message a node sent to another node, in the order they left.
    ///
    /// Some properties are about traffic rather than about what a node reports: how many messages
    /// a change costs, or who is still talking once things have settled. The journal has always
    /// recorded this; a checker simply had no way to reach it.
    pub sends: Vec<Sent>,
    pub crashed: HashMap<String, u64>,
    pub end: u64,
    /// True when the run was cut at the time limit instead of running out of events.
    ///
    /// Every liveness deadline dates from the end of the run, so judging a truncated run reports
    /// "never happened" when the truth is "we stopped looking". Worse, the events scheduled past
    /// the limit never fire at all: a workload asking for twelve requests may have exercised nine.
    pub truncated: bool,
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
    let mut ev = Events {
        observes: vec![],
        stimuli: vec![],
        sends: vec![],
        crashed: HashMap::new(),
        end: 0,
        truncated: false,
    };
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
            "send" => {
                let src = v.get("src").and_then(Value::as_str).unwrap_or("").to_string();
                let dst = v.get("dest").and_then(Value::as_str).unwrap_or("").to_string();
                let mid = v.get("mid").and_then(Value::as_u64).unwrap_or(0);
                ev.sends.push(Sent { t, src, dest: dst, mid, body });
            }
            "stimulus" => {
                let dst = v.get("dest").and_then(Value::as_str).unwrap_or("").to_string();
                ev.stimuli.push((t, dst, body));
            }
            "end" => {
                ev.truncated =
                    v.pointer("/detail/reason").and_then(Value::as_str) == Some("time-limit");
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

/// A parametric scenario: one scenario holding many runs, each one named by a seed.
///
/// One run proves nothing. The bugs worth catching show up on a minority of seeds: running once
/// has a 90% chance of concluding a broken implementation works.
///
/// It pairs an environment with a workload, and those pairs are listed rather than crossed,
/// because not every crossing is legal. An algorithm that assumes no crashes, paired with an
/// environment that injects them, deadlocks — correctly. Listing makes "this is never run with
/// crashes" one visible line instead of a property nobody rereads.
pub struct Parametric {
    /// The space, relative to the suite directory: the world a run happens in and the tree of
    /// events that befall it.
    pub space: &'static str,
    pub seeds: u64,
}

/// A stored scenario, run once, with the failures a *correct* implementation should produce.
///
/// `expect_fail` is not a formality. A scenario can exist to show what an algorithm's assumptions
/// cost when they do not hold, and a suite that could only say "all green" could not express one.
pub struct Written {
    /// Scenario path, relative to the suite directory.
    pub path: &'static str,
    pub expect_fail: &'static [&'static str],
    pub why: &'static str,
}

pub struct Suite {
    pub name: &'static str,
    pub parametric: &'static [Parametric],
    /// Scenarios written by hand. Immune to any change in the draw, which is why the cases worth
    /// keeping live here rather than as a seed.
    pub written: &'static [Written],
    pub check: fn(&Events, &Scenario, &mut Report),
    /// Where the caller keeps its cuesheet style sheet, if it has one, so that a failing seed can
    /// print the command that draws it.
    ///
    /// A path, never a vocabulary: the harness still has no idea what any of the words in that
    /// file mean, and never opens it. `None` prints nothing, which is the right answer for a
    /// caller that has no sheet, since under cuesheet's own rules the command would fail without
    /// one and a command that cannot work is worse than no command at all.
    pub style: Option<&'static str>,
}

// ------------------------------------------------------------------- running

struct Opts {
    suite_dir: PathBuf,
    out: PathBuf,
    seeds: Option<u64>,
    /// Inclusive seed range. Debugging one failure should not mean re-running the other 199.
    range: Option<(u64, u64)>,
    /// Substring selecting which parametric and written scenarios to run.
    only: Option<String>,
    list: bool,
    watchdog: u64,
    /// How many seeds run at once.
    jobs: usize,
    program: Vec<String>,
}

/// The name a written scenario is selected by: its file stem.
fn stem(path: &str) -> &str {
    Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("written")
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
    --seeds <n>        override every parametric scenario's seed count
    --seed <a>[..<b>]  run one seed, or an inclusive range, instead of every seed
    --only <name>      run only what matches: a parametric scenario's name or a written scenario's
    --watchdog <ms>    wall-clock hang detector      [default: 5000]
    --jobs <n>         seeds run at once             [default: half the cores]
    --list             show the parametric and scenarios this suite defines, then exit
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
        // Half the cores, rounded up. A simulation waits on one node at a time, but it starts a
        // process per node and those starts overlap, so one per core oversubscribes the machine.
        // Measured on twelve cores, 200 seeds of one space: 23.4 s at one job, 10.0 at four,
        // 8.9 at six, 8.5 at eight, then 10.8 at twelve — slower than eight, and no faster than
        // four. Half is within a few percent of the best and leaves the machine usable.
        jobs: std::thread::available_parallelism().map(|n| n.get().div_ceil(2)).unwrap_or(1),
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
            "--jobs" => {
                o.jobs = val(i)?.parse().map_err(|_| "bad --jobs")?;
                if o.jobs == 0 {
                    return Err("--jobs 0: at least one seed has to run at a time".into());
                }
                i += 2
            }
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

/// A parametric scenario is named by its coordinates. Two file names already say which workload ran in which
/// environment, and a label of its own would be a third name to keep true.
fn label(p: &Parametric) -> String {
    stem(p.space).to_string()
}

/// A parametric scenario's two spaces, parsed, plus what they said. Read once per parametric scenario rather than once per
/// seed: it is the same pair of files every time.
struct Spaces {
    space: Space,
    fp: String,
}

fn load_spaces(o: &Opts, p: &Parametric) -> Result<Spaces, String> {
    let read = |rel: &str| -> Result<String, String> {
        let path = o.suite_dir.join(rel);
        std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))
    };
    let src = read(p.space)?;
    Ok(Spaces { space: Space::from_json(&src)?, fp: fingerprint(&src) })
}

/// Where a drawn scenario came from, stored beside the journal so a seed can be read back as the
/// run it actually named.
fn provenance(p: &Parametric, sp: &Spaces, seed: u64) -> Drawn {
    Drawn { seed, space: p.space.to_string(), fingerprint: sp.fp.clone() }
}

/// Events a scenario schedules past its own time limit, which the run will never reach.
///
/// Not the same thing as a run that ends at the limit: an algorithm driven by timers, a failure
/// detector above all, keeps going until something stops it, and that is normal. This is a space
/// that asks for what cannot happen, and it would otherwise shrink the workload in silence.
fn past_the_limit(sc: &Scenario) -> usize {
    let late = |t: u64| t >= sc.time_limit;
    sc.stimuli.iter().filter(|s| late(s.at)).count()
        + sc.faults.iter().filter(|f| late(f.at())).count()
}

fn load_scenario(path: &Path) -> Result<Scenario, String> {
    let raw = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    Scenario::from_json(&raw)
}

/// Render the diagrams beside the journal, in both forms.
///
/// Only for runs worth opening: a sweep deletes the seeds that pass, and rendering hundreds of
/// diagrams nobody looks at would be waste.
///
/// Both, because they are good at different things and neither is a fallback for the other.
/// Mermaid renders natively in a GitHub issue or pull request, from a fenced block, with no file
/// and no build — which is exactly where a failing seed gets discussed. The cuesheet document
/// draws a message as a segment from the instant it left to the instant it landed, so its slope is
/// its flight time, which is the thing a horizontal arrow cannot say.
fn draw(dir: &Path) -> Vec<PathBuf> {
    let journal = dir.join("journal.jsonl");
    let mut out = Vec::new();

    let mmd = dir.join("messages.mmd");
    if cuelight::viz::render(&journal, &mmd, 200).is_ok() {
        out.push(mmd);
    }

    let st = dir.join("messages.cuesheet");
    let scenario = dir.join("scenario.json");
    let sc = scenario.exists().then_some(scenario);
    if cuelight::cuesheet::render(&journal, sc.as_deref(), &st, 200).is_ok() {
        out.push(st);
    }
    out
}

/// Where to read what happened. The run directory holds it all, but nobody guesses its path.
///
/// A path a reader cannot open is halfway to useless, so when the caller has named a style sheet
/// the command that turns the document into a picture is printed too, ready to paste. The syntax
/// is cuesheet's and is written here as a literal, which is a coupling worth naming: if that CLI
/// changes, this line goes stale with nothing to catch it. It is the same string JOURNAL.md
/// carries, so the two drift together rather than separately.
fn where_to_read(dir: &Path, style: Option<&str>) {
    let journal = dir.join("journal.jsonl");
    if journal.exists() {
        println!("    journal: {}", journal.display());
    }
    let drawn = draw(dir);
    for (i, d) in drawn.iter().enumerate() {
        let label = if i == 0 { "diagram" } else { "        " };
        println!("    {label}: {}", d.display());
    }
    if let Some(line) = render_hint(&drawn, style) {
        println!("    {line}");
    }
}

/// The paste-ready render command, or nothing.
///
/// Nothing in two cases, and they are different: the caller named no style sheet, or no document
/// was drawn. Under cuesheet's own rule a render without a sheet is an error, so printing the
/// command anyway would hand the reader something that cannot work, which is worse than handing
/// them nothing.
fn render_hint(drawn: &[PathBuf], style: Option<&str>) -> Option<String> {
    let style = style?;
    let doc = drawn.iter().find(|d| d.extension().is_some_and(|e| e == "cuesheet"))?;
    Some(format!("render:  cuesheet render {} --style {style} --open", doc.display()))
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

/// Run and judge one seed of one parametric scenario. `None` is a pass; otherwise, why not.
fn one_seed(o: &Opts, suite: &Suite, p: &Parametric, sp: &Spaces, name_of: &str, seed: u64) -> Option<String> {
    let dir = o.out.join(name_of).join(seed.to_string());
    let sc = Scenario::draw(seed, &sp.space).from(provenance(p, sp, seed));
    let late = past_the_limit(&sc);
    if late > 0 {
        return Some(format!("{late} event(s) scheduled past the time limit; they cannot happen"));
    }
    if let Err(e) = run_once(o, &dir, sc.clone()) {
        return Some(e);
    }
    let ev = match load_events(&dir) {
        Ok(ev) => ev,
        Err(e) => return Some(e),
    };
    let mut r = Report::default();
    (suite.check)(&ev, &sc, &mut r);
    if r.ok() {
        let _ = std::fs::remove_dir_all(&dir); // keep only failures
        None
    } else {
        Some(r.failed().join(", "))
    }
}

/// Apply `f` to every seed of `lo..=hi`, at most `jobs` at a time, and return the results in seed
/// order whatever order they finished in.
///
/// The concurrency is between simulations, never inside one: a simulation stays a single thread
/// advancing logical time, which is the whole reason this tool exists.
fn each_seed<T: Send>(lo: u64, hi: u64, jobs: usize, f: impl Fn(u64) -> T + Sync) -> Vec<(u64, T)> {
    let next = std::sync::atomic::AtomicU64::new(lo);
    let done = std::sync::Mutex::new(Vec::new());
    let workers = (jobs.max(1) as u64).min(hi.saturating_sub(lo) + 1);
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| loop {
                let seed = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if seed > hi {
                    break;
                }
                let out = f(seed);
                done.lock().unwrap().push((seed, out));
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_by_key(|(seed, _)| *seed);
    done
}

/// Replay one case and compare the two journals, before judging anything.
///
/// The error carries its own banner, because two very different things fail here and a newcomer
/// meets the other one first: a node that is not written yet dies on its first event, and calling
/// that "not deterministic" sends them looking for a clock they never used.
///
/// A node that reads the system clock, spawns a thread or draws unseeded randomness produces a
/// different run every time. Every verdict below would then describe a run nobody can reproduce,
/// and the replay command printed under each failure would replay something else. So this runs
/// first, and stops the suite when it fails, rather than appearing as one line among the results.
///
/// Two runs, against several hundred: the cost is invisible.
fn replays_identically(o: &Opts, suite: &Suite) -> Result<String, String> {
    // A drawn scenario from the first parametric scenario, not the first written one. A written scenario is
    // usually written *because* it is degenerate, and replaying one where every node dies at t=1
    // compares two empty journals: the check passes without having exercised anything, and a node
    // that reads the clock then collects a green verdict.
    let (what, sc) = match suite.parametric.first() {
        Some(p) => {
            let sp = load_spaces(o, p).map_err(|e| format!("FAILED: {e}"))?;
            (
                format!("{} seed 1", label(p)),
                Scenario::draw(1, &sp.space).from(provenance(p, &sp, 1)),
            )
        }
        None => match suite.written.first() {
            Some(w) => (
                stem(w.path).to_string(),
                load_scenario(&o.suite_dir.join(w.path)).map_err(|e| format!("FAILED: {e}"))?,
            ),
            None => return Ok(String::new()),
        },
    };

    let mut runs = vec![];
    for pass in ["a", "b"] {
        let dir = o.out.join("replay").join(pass);
        run_once(o, &dir, sc.clone()).map_err(|e| format!("FAILED: {e}"))?;
        let j = dir.join("journal.jsonl");
        let text =
            std::fs::read_to_string(&j).map_err(|e| format!("FAILED: {}: {e}", j.display()))?;
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
        "NOT DETERMINISTIC: replaying {what} does not give the same journal twice.\n\
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
            .parametric
            .iter()
            .map(|p| label(p).len())
            .chain(suite.written.iter().map(|x| stem(x.path).len()))
            .max()
            .unwrap_or(0)
            .max(8); // a floor, so a suite with one short name still reads as a column
        println!("parametric:");
        for p in suite.parametric {
            println!("  {:<w$} {} seeds from {}", label(p), p.seeds, p.space);
        }
        println!("written:");
        for x in suite.written {
            println!("  {:<w$} {}", stem(x.path), x.why);
        }
        return ExitCode::SUCCESS;
    }

    if o.program.is_empty() {
        eprintln!("error: missing --bin\n\n{}", usage(&name));
        return ExitCode::FAILURE;
    }

    // Nothing matched is a mistake, not an empty run: silently doing nothing looks like success.
    if let Some(pat) = &o.only {
        let known = suite.parametric.iter().any(|p| label(p).contains(pat.as_str()))
            || suite.written.iter().any(|x| stem(x.path).contains(pat.as_str()));
        if !known {
            eprintln!("--only {pat} matches nothing. `--list` shows what this suite defines.");
            return ExitCode::FAILURE;
        }
    }

    // First, and on its own: nothing below means anything if the node is not deterministic.
    match replays_identically(&o, &suite) {
        Ok(what) if what.is_empty() => {}
        Ok(what) => println!("deterministic: replaying {what} gives the same journal twice\n"),
        Err(e) => { eprintln!("{e}"); return ExitCode::FAILURE }
    }

    let mut all_ok = true;

    for p in suite.parametric {
        let name_of = label(p);
        if let Some(pat) = &o.only {
            if !name_of.contains(pat.as_str()) {
                continue;
            }
        }
        let sp = match load_spaces(&o, p) {
            Ok(x) => x,
            Err(e) => { println!("{name_of}: {e}"); all_ok = false; continue }
        };

        let seeds = o.seeds.unwrap_or(p.seeds);
        let (lo, hi) = o.range.unwrap_or((1, seeds));
        // Runs share nothing: each has its own seed, its own node processes and its own
        // directory. So they can overlap without any of them noticing, and determinism, which is
        // a property of one run, is untouched. What must not change is what gets *printed*: the
        // verdicts are collected and then read in seed order, so the first five failures shown
        // are the five lowest whatever finished first.
        let verdicts = each_seed(lo, hi, o.jobs, |seed| one_seed(&o, &suite, p, &sp, &name_of, seed));
        let pass = verdicts.iter().filter(|(_, v)| v.is_none()).count() as u64;
        let fails: Vec<(u64, String)> =
            verdicts.into_iter().filter_map(|(seed, v)| v.map(|why| (seed, why))).collect();
        println!("{name_of}: {pass}/{} seeds passed{}", hi - lo + 1,
                 if o.range.is_some() { format!(" (seeds {lo}..{hi})") } else { String::new() });
        for (s, e) in fails.iter().take(5) {
            // The address of a failure is the triple, not the seed: a seed names a run only
            // relative to the two spaces it was drawn from.
            println!("  seed {s}: {e}");
            how_to_replay(&name, &o, &format!("--seed {s} --only {name_of}"));
            where_to_read(&o.out.join(&name_of).join(s.to_string()), suite.style);
        }
        if fails.len() > 5 {
            println!("  ... and {} more", fails.len() - 5);
        }
        all_ok &= fails.is_empty();
    }

    for d in suite.written {
        // A written scenario has no seed, so asking for one means asking for drawn runs only.
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
        let dir = o.out.join("written").join(stem);
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
                    // Expected-failure is the point of some written scenarios: a suite that could
                    // only say "all green" could not express them.
                    if got == d.expect_fail {
                        println!("  → as expected");
                    } else {
                        println!("  → EXPECTED {:?}, GOT {:?}", d.expect_fail, got);
                        how_to_replay(&name, &o, &format!("--only {stem}"));
                        all_ok = false;
                    }
                    // Always, even when it behaved: a written scenario is often run precisely to
                    // be looked at, and the diagram is the reason to look.
                    where_to_read(&dir, suite.style);
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

    /// A run that ends at the limit is normal, and a space that schedules past it is not: those
    /// events never happen, so the workload shrinks in silence.
    #[test]
    fn events_past_the_limit_are_counted() {
        let inside = sc(r#"{"time_limit": 100, "stimuli": [{"at": 99, "node": "n0", "body": {}}]}"#);
        assert_eq!(past_the_limit(&inside), 0);
        let outside = sc(r#"{"time_limit": 100,
            "stimuli": [{"at": 100, "node": "n0", "body": {}}, {"at": 400, "node": "n1", "body": {}}],
            "faults": [{"kind": "crash", "at": 250, "node": "n2"}]}"#);
        assert_eq!(past_the_limit(&outside), 3);
    }

    /// The group size is no longer the suite's business: it follows from `f` in the environment
    /// space, and is tested there. What is this crate's business is that a parametric scenario can be named,
    /// because that name addresses a run directory and a `--only` selector.
    #[test]
    fn a_pairing_is_named_by_its_coordinates() {
        let p = Parametric { space: "spaces/mutex-no-crash.json", seeds: 1 };
        assert_eq!(label(&p), "mutex-no-crash");
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

    use super::*;

    fn drawn() -> Vec<PathBuf> {
        vec![PathBuf::from("store/suite/sweep/7/messages.mmd"),
             PathBuf::from("store/suite/sweep/7/messages.cuesheet")]
    }

    /// The whole point: a path a reader cannot open, plus the line that opens it.
    #[test]
    fn a_named_style_sheet_produces_a_command_that_can_be_pasted() {
        let line = render_hint(&drawn(), Some("cuesheet.cuestyle")).expect("a hint");
        assert_eq!(
            line,
            "render:  cuesheet render store/suite/sweep/7/messages.cuesheet \
             --style cuesheet.cuestyle --open"
                .replace("             ", "")
        );
    }

    /// A caller with no sheet gets no command. cuesheet refuses to render without one, so the line
    /// would be an instruction to fail.
    #[test]
    fn no_style_sheet_means_no_command() {
        assert_eq!(render_hint(&drawn(), None), None);
    }

    /// The same, on a run that really happened: three nodes greeting one another. Every `send`
    /// line of the journal is handed over, numbered as the journal numbers it.
    #[test]
    fn the_sends_of_a_real_run_are_all_there() {
        let python = std::process::Command::new("python3").arg("--version").output();
        if !python.map(|o| o.status.success()).unwrap_or(false) {
            return;
        }
        let node = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../testdata/chatter.py");
        let argv = vec!["--bin".to_string(), "python3".to_string(), node.display().to_string()];
        let o = parse(&argv, "t", ".").expect("options");
        let dir = std::env::temp_dir().join("cuelight-suite-test-real-sends");
        let _ = std::fs::remove_dir_all(&dir);
        run_once(&o, &dir, sc(r#"{"nodes": 3, "time_limit": 400}"#)).expect("the run");
        let ev = load_events(&dir).expect("events");

        let journal = std::fs::read_to_string(dir.join("journal.jsonl")).unwrap();
        let written = journal.lines().filter(|l| l.contains(r#""kind":"send""#)).count();
        assert!(written >= 6, "three nodes greet two peers each, at the very least");
        assert_eq!(ev.sends.len(), written);
        let mids: Vec<u64> = ev.sends.iter().map(|m| m.mid).collect();
        assert_eq!(mids, (1..=written as u64).collect::<Vec<_>>(), "dense from 1, in the order they left");
        assert!(ev.sends.iter().all(|m| m.src != m.dest && m.dest != "harness"));
        assert!(ev.sends.windows(2).all(|w| w[0].t <= w[1].t));
    }

    /// Whatever finishes first, results come back in seed order, each seed exactly once.
    #[test]
    fn seeds_run_at_once_still_come_back_in_order() {
        for jobs in [1, 3, 16] {
            let got = each_seed(5, 40, jobs, |seed| {
                // The low seeds are the slow ones, so they finish last when there is room to overlap.
                std::thread::sleep(std::time::Duration::from_millis(if seed < 10 { 3 } else { 0 }));
                seed * 2
            });
            let want: Vec<(u64, u64)> = (5..=40).map(|s| (s, s * 2)).collect();
            assert_eq!(got, want, "with {jobs} job(s)");
        }
        assert_eq!(each_seed(7, 7, 8, |s| s), vec![(7, 7)]);
    }

    /// A checker can count traffic: every message between two nodes is handed over, in the order
    /// they left, and what a node says to the harness is not one of them.
    #[test]
    fn the_sends_of_a_run_reach_the_checker() {
        let dir = std::env::temp_dir().join("cuelight-suite-test-sends");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let journal = [
            r#"{"seq":0,"t":3,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"hello"}}"#,
            r#"{"seq":1,"t":4,"kind":"observe","src":"n0","dest":"harness","body":{"type":"said"}}"#,
            r#"{"seq":2,"t":9,"kind":"recv","src":"n0","dest":"n1","mid":1,"body":{"type":"hello"}}"#,
            r#"{"seq":3,"t":9,"kind":"send","src":"n1","dest":"n0","mid":2,"body":{"type":"hi"}}"#,
        ]
        .join("\n");
        std::fs::write(dir.join("journal.jsonl"), journal).unwrap();
        let ev = load_events(&dir).expect("events");
        let seen: Vec<(u64, &str, &str, u64, &str)> = ev
            .sends
            .iter()
            .map(|m| (m.t, m.src.as_str(), m.dest.as_str(), m.mid, ty(&m.body)))
            .collect();
        assert_eq!(seen, vec![(3, "n0", "n1", 1, "hello"), (9, "n1", "n0", 2, "hi")]);
        assert_eq!(ev.observes.len(), 1);
    }

    /// And no document means no command either, whatever the caller named.
    #[test]
    fn no_document_means_no_command() {
        let only_mermaid = vec![PathBuf::from("store/suite/sweep/7/messages.mmd")];
        assert_eq!(render_hint(&only_mermaid, Some("cuesheet.cuestyle")), None);
    }
}
