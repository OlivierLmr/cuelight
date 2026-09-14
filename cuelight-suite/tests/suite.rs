//! End-to-end tests of the suite machinery, through the `demo` example and real node processes.
//!
//! This crate decides what runs, in what order, and what is printed under a failure, and none of
//! that is visible from a unit test. The fixtures in `testdata/` are Python nodes, so these are
//! skipped when python3 is unavailable rather than failing someone touching only the Rust.

use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn have_python() -> bool {
    Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// The example is not a `[[bin]]`, so cargo exports no path for it. Build it once, then find it
/// beside the test executable.
fn demo() -> &'static PathBuf {
    static P: OnceLock<PathBuf> = OnceLock::new();
    P.get_or_init(|| {
        let built = Command::new(env!("CARGO"))
            .args(["build", "-p", "cuelight-suite", "--example", "demo"])
            .current_dir(root())
            .status()
            .expect("cargo did not start");
        assert!(built.success(), "the demo example did not build");
        let exe = std::env::current_exe().expect("no test executable");
        let target = exe.parent().and_then(|p| p.parent()).expect("unexpected layout");
        let p = target.join("examples").join("demo");
        assert!(p.exists(), "no demo example at {}", p.display());
        p
    })
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cuelight-suite-test-{name}"));
    let _ = std::fs::remove_dir_all(&p);
    p
}

/// Run the demo suite against one fixture node. `--bin` swallows the rest of the line, so
/// everything else has to come first.
fn demo_run(out: &str, node: &str, extra: &[&str]) -> (bool, String) {
    let o = Command::new(demo())
        .arg("--out")
        .arg(tmp(out))
        .args(extra)
        .arg("--bin")
        .arg("python3")
        .arg(root().join("testdata").join(node))
        .current_dir(root())
        .output()
        .expect("the demo suite did not start");
    let mut text = String::from_utf8_lossy(&o.stdout).into_owned();
    text.push_str(&String::from_utf8_lossy(&o.stderr));
    (o.status.success(), text)
}

#[test]
fn list_names_each_pairing_by_its_coordinates() {
    let o = Command::new(demo()).arg("--list").current_dir(root()).output().expect("no run");
    let text = String::from_utf8_lossy(&o.stdout);
    // A workload and an environment give both halves; an environment alone names itself.
    assert!(text.contains("pokes-quiet"), "{text}");
    assert!(text.contains("\n  crashes "), "{text}");
    assert!(text.contains("nobody-home"), "{text}");
}

#[test]
fn a_node_that_answers_passes_every_pairing() {
    if !have_python() {
        return;
    }
    let (ok, text) = demo_run("green", "poked.py", &[]);
    assert!(ok, "{text}");
    assert!(text.contains("pokes-quiet: 4/4 seeds passed"), "{text}");
    // A parametric scenario with no workload is a parametric scenario all the same.
    assert!(text.contains("crashes: 4/4 seeds passed"), "{text}");
    assert!(text.contains("all good"), "{text}");
}

#[test]
fn a_written_scenario_may_be_expected_to_fail() {
    if !have_python() {
        return;
    }
    let (ok, text) = demo_run("expected", "poked.py", &[]);
    // Its whole point: a suite that could only say "all green" could not express this one.
    assert!(text.contains("→ as expected"), "{text}");
    assert!(ok, "an expected failure must not fail the suite: {text}");
}

#[test]
fn a_failure_is_reported_with_the_command_that_reproduces_it() {
    if !have_python() {
        return;
    }
    let (ok, text) = demo_run("red", "silent.py", &[]);
    assert!(!ok, "a silent node passed: {text}");
    assert!(text.contains("pokes-quiet: 0/4 seeds passed"), "{text}");
    assert!(text.contains("every-poke-answered"), "{text}");
    // The address of a failure is the triple, and the replay line has to carry all of it.
    assert!(text.contains("--seed 1 --only pokes-quiet"), "{text}");
    assert!(text.contains("journal:"), "{text}");
}

#[test]
fn a_node_that_does_not_replay_stops_the_suite_before_any_verdict() {
    if !have_python() {
        return;
    }
    let (ok, text) = demo_run("flaky", "flaky.py", &[]);
    assert!(!ok, "a node reading the clock passed: {text}");
    assert!(text.contains("NOT DETERMINISTIC"), "{text}");
    // Nothing may be judged: every verdict below would describe a run nobody can reproduce.
    assert!(!text.contains("seeds passed"), "it judged anyway: {text}");
    // And the case it replays must be one that exercises something. A written scenario is usually
    // written because it is degenerate; this suite's kills both nodes at t=1, and replaying it
    // compares two empty journals.
    assert!(text.contains("pokes-quiet seed 1"), "it replayed the wrong case: {text}");
}

/// The first thing anyone sees, and it used to name the wrong thing: an unfilled skeleton dies on
/// its first event, which is not a replay failure. Calling it one sends a newcomer hunting for a
/// clock they never used.
#[test]
fn a_skeleton_nobody_has_filled_in_fails_without_blaming_determinism() {
    if !have_python() {
        return;
    }
    let (ok, text) = demo_run("unwritten", "unwritten.py", &[]);
    assert!(!ok, "{text}");
    assert!(text.contains("exited on its own"), "{text}");
    assert!(!text.contains("NOT DETERMINISTIC"), "a dead node is not a clock read: {text}");
    assert!(text.contains("FAILED"), "{text}");
}

#[test]
fn only_runs_what_it_names_and_refuses_what_it_does_not() {
    if !have_python() {
        return;
    }
    let (ok, text) = demo_run("only", "poked.py", &["--only", "crashes"]);
    assert!(ok, "{text}");
    assert!(text.contains("crashes: 4/4"), "{text}");
    assert!(!text.contains("pokes-quiet:"), "--only ran a parametric scenario it did not name: {text}");
    assert!(!text.contains("nobody-home.json:"), "--only ran a written scenario too: {text}");

    // Matching nothing is a mistake, not an empty run: doing nothing quietly looks like success.
    let (ok, text) = demo_run("nomatch", "poked.py", &["--only", "does-not-exist"]);
    assert!(!ok, "{text}");
    assert!(text.contains("matches nothing"), "{text}");
}

#[test]
fn one_seed_runs_one_seed_and_leaves_the_written_scenarios_alone() {
    if !have_python() {
        return;
    }
    let (ok, text) = demo_run("oneseed", "poked.py", &["--seed", "2"]);
    assert!(ok, "{text}");
    assert!(text.contains("1/1 seeds passed (seeds 2..2)"), "{text}");
    // Asking for a seed means asking for drawn runs: a written scenario has none.
    assert!(!text.contains("→ as expected"), "{text}");
}
