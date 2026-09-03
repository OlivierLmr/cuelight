//! End-to-end tests: the real binary, real node processes, real journals.
//!
//! Everything here goes through `cuelight` the way a user does — no internals. The fixtures in
//! `testdata/` are Python nodes, so these are skipped when python3 is unavailable rather than
//! failing someone who is only touching the Rust.

use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_cuelight");

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn have_python() -> bool {
    Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Run `cuelight run` with the given args plus a fixture, into `out`. Returns (success, stderr).
fn run(out: &Path, args: &[&str], fixture: &str) -> (bool, String) {
    let o = Command::new(BIN)
        .arg("run")
        .args(args)
        .arg("--out")
        .arg(out)
        .arg("--bin")
        .arg("python3")
        .arg(root().join("testdata").join(fixture))
        .current_dir(root())
        .output()
        .expect("cuelight did not start");
    (o.status.success(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn journal(out: &Path) -> String {
    std::fs::read_to_string(out.join("journal.jsonl")).expect("no journal")
}

fn tmp(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("cuelight-test-{name}"));
    let _ = std::fs::remove_dir_all(&p);
    p
}

/// The property the whole tool exists for: a scenario replays identically.
#[test]
fn a_run_replays_byte_identical() {
    if !have_python() {
        return;
    }
    let (a, b) = (tmp("replay-a"), tmp("replay-b"));
    assert!(run(&a, &["--seed", "5", "--fifo"], "chatter.py").0);
    assert!(run(&b, &["--seed", "5", "--fifo"], "chatter.py").0);
    assert_eq!(journal(&a), journal(&b), "same scenario, different journal");
}

/// `check` is what catches wall-clock reads and threads in a node. It must agree with the above.
#[test]
fn check_reports_determinism() {
    if !have_python() {
        return;
    }
    let out = tmp("check");
    let o = Command::new(BIN)
        .args(["check", "--seed", "2", "--out"])
        .arg(&out)
        .arg("--bin")
        .arg("python3")
        .arg(root().join("testdata/chatter.py"))
        .current_dir(root())
        .output()
        .unwrap();
    assert!(o.status.success());
    assert!(String::from_utf8_lossy(&o.stdout).contains("DETERMINISTIC"));
}

/// `--fifo` must actually order the link, and its absence must actually reorder — otherwise the
/// flag could be a silent no-op and Lamport's mutex would appear to work without ordered channels.
#[test]
fn fifo_orders_a_link_and_its_absence_does_not() {
    if !have_python() {
        return;
    }
    let deliveries = |out: &Path| -> Vec<i64> {
        journal(out)
            .lines()
            .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
            .filter(|v| v["kind"] == "observe" && v["body"]["type"] == "deliver")
            .filter_map(|v| v["body"]["mid"].as_i64())
            .collect()
    };

    let ordered = tmp("fifo-on");
    assert!(run(&ordered, &["--seed", "1", "--no-faults", "--fifo"], "ordering.py").0);
    let got = deliveries(&ordered);
    assert!(!got.is_empty(), "the fixture delivered nothing");
    assert!(got.windows(2).all(|w| w[0] < w[1]), "--fifo left the link unordered: {got:?}");

    // Without it, at least one seed must scramble the same burst, or jitter is not doing its job.
    let scrambled = (1..15).any(|seed| {
        let out = tmp(&format!("fifo-off-{seed}"));
        run(&out, &["--seed", &seed.to_string(), "--no-faults"], "ordering.py").0
            && !deliveries(&out).windows(2).all(|w| w[0] < w[1])
    });
    assert!(scrambled, "no seed reordered the link: jitter cannot reorder, so --fifo is a no-op");
}

/// A node that exits on its own fails the run. The tool knows which crashes it injected, and a
/// program dying on unexpected input must not pass as having survived them.
#[test]
fn a_node_that_exits_on_its_own_fails_the_run() {
    if !have_python() {
        return;
    }
    let script = std::env::temp_dir().join("cuelight-test-quitter.py");
    std::fs::write(&script, "import sys\nsys.stdin.readline()\n").unwrap();
    let out = tmp("quitter");
    let o = Command::new(BIN)
        .args(["run", "--seed", "1", "--no-faults", "--out"])
        .arg(&out)
        .arg("--bin")
        .arg("python3")
        .arg(&script)
        .current_dir(root())
        .output()
        .unwrap();
    assert!(!o.status.success(), "a node died and the run still passed");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("exited on its own"), "unhelpful message: {err}");
}

/// Crashes land at the instant the scenario names, and a crashed sender's in-flight messages are
/// dropped — which is what makes a *partial* broadcast expressible at all.
#[test]
fn a_crash_lands_on_time_and_drops_what_was_in_flight() {
    if !have_python() {
        return;
    }
    let scenario = std::env::temp_dir().join("cuelight-test-crash.json");
    std::fs::write(
        &scenario,
        r#"{"nodes": 4, "gst": 0, "delay_pre_default": 500, "delay_post_default": 500,
            "faults": [{"kind": "crash", "at": 100, "node": "n0"}]}"#,
    )
    .unwrap();
    let out = tmp("crash");
    let o = Command::new(BIN)
        .args(["run", "--scenario"])
        .arg(&scenario)
        .arg("--out")
        .arg(&out)
        .arg("--bin")
        .arg("python3")
        .arg(root().join("testdata/chatter.py"))
        .current_dir(root())
        .output()
        .unwrap();
    assert!(o.status.success());
    let lines: Vec<serde_json::Value> = journal(&out)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let crash = lines
        .iter()
        .find(|v| v["kind"] == "fault-crash")
        .expect("no fault-crash entry");
    assert_eq!(crash["t"], 100, "the crash did not land at the stated instant");
    assert_eq!(crash["detail"]["node"], "n0");
    assert!(
        lines.iter().any(|v| v["kind"] == "drop-from-crashed"),
        "n0 died mid-flight with 500-unit links and nothing was dropped"
    );
}

/// The journal is a contract: dense `seq` from 0, non-decreasing `t`, `end` last, `done` absent.
#[test]
fn the_journal_keeps_its_contract() {
    if !have_python() {
        return;
    }
    let out = tmp("contract");
    assert!(run(&out, &["--seed", "8", "--fifo"], "chatter.py").0);
    let lines: Vec<serde_json::Value> = journal(&out)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(!lines.is_empty());
    for (i, v) in lines.iter().enumerate() {
        assert_eq!(v["seq"], i as i64, "seq is not dense from 0");
    }
    assert!(
        lines.windows(2).all(|w| w[0]["t"].as_u64() <= w[1]["t"].as_u64()),
        "t went backwards"
    );
    assert_eq!(lines.last().unwrap()["kind"], "end", "end is not the last entry");
    assert!(
        !lines.iter().any(|v| v["body"]["type"] == "done"),
        "`done` is a barrier, not an event: it must not be journalled"
    );
}

/// A run directory is the unit a checker reads: journal, scenario, one stderr per node.
#[test]
fn a_run_directory_holds_what_a_checker_needs() {
    if !have_python() {
        return;
    }
    let out = tmp("dir");
    assert!(run(&out, &["--seed", "1", "--no-faults"], "chatter.py").0);
    assert!(out.join("journal.jsonl").is_file());
    assert!(out.join("scenario.json").is_file(), "scenario.json is part of the contract");
    for i in 0..4 {
        assert!(out.join(format!("n{i}.stderr")).is_file(), "missing n{i}.stderr");
    }
}
