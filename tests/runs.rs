//! End-to-end tests: the real binary, real node processes, real journals.
//!
//! Everything here goes through `cuelight` the way a user does, never its internals. The fixtures in
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
    assert!(run(&a, &["--seed", "5", "--space", "testdata/env-fifo.json"], "chatter.py").0);
    assert!(run(&b, &["--seed", "5", "--space", "testdata/env-fifo.json"], "chatter.py").0);
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

/// An environment asking for FIFO must actually order the link, and one that does not must
/// actually reorder. Otherwise the field could be a silent no-op and Lamport's mutex would appear
/// to work without ordered channels.
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
    assert!(run(&ordered, &["--seed", "1", "--space", "testdata/env-fifo-clean.json"], "ordering.py").0);
    let got = deliveries(&ordered);
    assert!(!got.is_empty(), "the fixture delivered nothing");
    assert!(got.windows(2).all(|w| w[0] < w[1]), "fifo: true left the link unordered: {got:?}");

    // Without it, at least one seed must scramble the same burst, or jitter is not doing its job.
    let scrambled = (1..15).any(|seed| {
        let out = tmp(&format!("fifo-off-{seed}"));
        run(&out, &["--seed", &seed.to_string()], "ordering.py").0
            && !deliveries(&out).windows(2).all(|w| w[0] < w[1])
    });
    assert!(scrambled, "no seed reordered the link: jitter cannot reorder, so fifo is a no-op");
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
        .args(["run", "--seed", "1", "--out"])
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
/// A crash lands when the scenario says, and stops the sends its victim had not got out yet.
///
/// A process does not hand every message to the network at once. Dying inside that loop is what
/// leaves a broadcast half-delivered, which is the whole reason reliable broadcast exists. What a
/// crash must *not* do is unsend: a message already gone still arrives, however slow the link.
#[test]
fn a_crash_stops_the_sends_its_victim_had_not_made_yet() {
    if !have_python() {
        return;
    }
    let scenario = std::env::temp_dir().join("cuelight-test-crash.json");
    // A wide emit gap and a crash a few ticks in, so the victim gets some of its sends out and not
    // the rest. Links are slow, so whatever did leave is still in flight when it dies.
    std::fs::write(
        &scenario,
        r#"{"nodes": 4, "gst": 0, "delay_pre_default": 500, "delay_post_default": 500,
            "emit_gap": 40, "faults": [{"kind": "crash", "at": 30, "node": "n0"}]}"#,
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
    let lines: Vec<serde_json::Value> =
        journal(&out).lines().filter_map(|l| serde_json::from_str(l).ok()).collect();

    let crash = lines.iter().find(|v| v["kind"] == "fault-crash").expect("no fault-crash entry");
    assert_eq!(crash["t"], 30, "the crash did not land at the stated instant");
    assert_eq!(crash["detail"]["node"], "n0");

    assert!(
        lines.iter().any(|v| v["kind"] == "never-sent" && v["detail"]["src"] == "n0"),
        "n0 died partway through its send loop and every message still left"
    );
    // And the ones that did leave were not unsent: each was delivered, 500 ticks later, long after
    // the crash.
    let sent = lines.iter().filter(|v| v["kind"] == "send" && v["src"] == "n0").count();
    let landed = lines.iter().filter(|v| v["kind"] == "recv" && v["src"] == "n0").count();
    assert!(sent > 0, "n0 got nothing out at all");
    assert_eq!(landed, sent, "{sent} left n0 before it died, {landed} arrived");
}

/// The journal is a contract: dense `seq` from 0, non-decreasing `t`, `end` last, `done` absent.
#[test]
fn the_journal_keeps_its_contract() {
    if !have_python() {
        return;
    }
    let out = tmp("contract");
    assert!(run(&out, &["--seed", "8", "--space", "testdata/env-fifo.json"], "chatter.py").0);
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
    assert!(run(&out, &["--seed", "1"], "chatter.py").0);
    assert!(out.join("journal.jsonl").is_file());
    assert!(out.join("scenario.json").is_file(), "scenario.json is part of the contract");
    for i in 0..4 {
        assert!(out.join(format!("n{i}.stderr")).is_file(), "missing n{i}.stderr");
    }
}

// ---------------------------------------------------------------------------
// The cuesheet document: a real run, through the real binary
// ---------------------------------------------------------------------------

/// A scenario with all three faults in it, written by hand so the run is the same every time.
const FAULTY: &str = r#"{
  "nodes": 4, "f": 1, "gst": 400, "time_limit": 3000, "fifo": false,
  "jitter_pct": 100, "emit_gap": 6,
  "faults": [
    { "kind": "pause",     "at": 120, "node": "n3", "duration": 140 },
    { "kind": "partition", "at": 260, "duration": 240, "side": ["n0", "n2"] },
    { "kind": "crash",     "at": 700, "node": "n1" }
  ],
  "stimuli": [] }"#;

/// Run the faulty scenario and hand back the document `--format cuesheet` wrote.
fn document(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("cuelight-doc-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sc = dir.join("in.json");
    std::fs::write(&sc, FAULTY).unwrap();

    let run = Command::new(BIN)
        .current_dir(root())
        .args(["run", "--scenario", sc.to_str().unwrap(), "--out", dir.to_str().unwrap()])
        .args(["--bin", "python3", "testdata/chatter.py"])
        .output()
        .expect("cuelight runs");
    assert!(run.status.success(), "{}", String::from_utf8_lossy(&run.stderr));

    let viz = Command::new(BIN)
        .current_dir(root())
        .args(["viz", "--journal", dir.join("journal.jsonl").to_str().unwrap()])
        .args(["--out", dir.to_str().unwrap(), "--format", "cuesheet"])
        .output()
        .expect("cuelight viz runs");
    assert!(viz.status.success(), "{}", String::from_utf8_lossy(&viz.stderr));

    std::fs::read_to_string(dir.join("messages.st")).expect("a document was written")
}

#[test]
fn a_run_becomes_a_document_that_names_every_process_and_every_fault() {
    if !have_python() {
        return;
    }
    let d = document("shape");

    assert!(d.contains("participants n0 n1 n2 n3"), "{d}");
    // Each fault, in the shape the renderer derives from.
    assert!(d.contains("120 n3 paused @260"), "the pause is missing:\n{d}");
    assert!(d.contains("260 network split n0 n2 @500"), "the partition is missing");
    assert!(d.contains("700 n1 crash"), "the crash is missing");
    assert!(d.contains("run end quiescent"), "the end is missing");
    // The protocol's own words, with the journal's categorisation as a class.
    assert!(d.contains(".hello") && d.contains(".ack"), "message types are missing");
    assert!(d.contains("deliver .observe"), "observations are missing");
}

/// Every arrow says when it left and when it landed, which is what makes the slope mean something.
#[test]
fn every_arrow_in_the_document_carries_two_instants() {
    if !have_python() {
        return;
    }
    let d = document("arrows");
    let mut arrows = 0;
    for line in d.lines() {
        if !line.contains(" -> ") {
            continue;
        }
        arrows += 1;
        let last = line.split_whitespace().last().unwrap();
        assert!(
            last.starts_with('@') || last.starts_with('+'),
            "an arrow with no arrival: {line}"
        );
        // And it lands no earlier than it left.
        let left: i64 = line.split_whitespace().next().unwrap().parse().unwrap();
        if let Some(at) = last.strip_prefix('@') {
            let landed: i64 = at.parse().unwrap();
            assert!(landed >= left, "a message landed before it left: {line}");
        }
    }
    assert!(arrows > 40, "expected a busy run, got {arrows} arrows");
}

/// The document is a function of the journal and nothing else, so a regenerated diagram diffs
/// against the one before it.
#[test]
fn the_same_run_produces_the_same_document() {
    if !have_python() {
        return;
    }
    assert_eq!(document("det-a"), document("det-b"));
}

/// Both forms, because they are good at different things and neither is a fallback for the other.
#[test]
fn viz_writes_mermaid_by_default_and_cuesheet_when_asked() {
    if !have_python() {
        return;
    }
    let dir = std::env::temp_dir().join("cuelight-doc-both");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sc = dir.join("in.json");
    std::fs::write(&sc, FAULTY).unwrap();
    Command::new(BIN)
        .current_dir(root())
        .args(["run", "--scenario", sc.to_str().unwrap(), "--out", dir.to_str().unwrap()])
        .args(["--bin", "python3", "testdata/chatter.py"])
        .output()
        .unwrap();

    let journal = dir.join("journal.jsonl");
    for (args, file, head) in [
        (vec![], "messages.mmd", "%%{init"),
        (vec!["--format", "cuesheet"], "messages.st", "#"),
    ] {
        let o = Command::new(BIN)
            .current_dir(root())
            .args(["viz", "--journal", journal.to_str().unwrap()])
            .args(["--out", dir.to_str().unwrap()])
            .args(&args)
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let body = std::fs::read_to_string(dir.join(file)).expect(file);
        assert!(body.starts_with(head), "{file} began with {:?}", &body[..20.min(body.len())]);
    }
}

#[test]
fn an_unknown_format_names_the_two_that_exist() {
    let o = Command::new(BIN)
        .current_dir(root())
        .args(["viz", "--journal", "whatever.jsonl", "--format", "postscript"])
        .output()
        .unwrap();
    assert!(!o.status.success());
    let e = String::from_utf8_lossy(&o.stderr);
    assert!(e.contains("mermaid") && e.contains("cuesheet"), "{e}");
}

/// The provenance a figure in a handout needs to name the run behind it, and the GST trap: the
/// scheduled instant is not the effective one when a fault outlasts it.
#[test]
fn the_document_carries_provenance_and_both_gst_instants() {
    if !have_python() {
        return;
    }
    let d = document("provenance");
    // The run directory's own copy, which is the canonical replay unit, not whatever file the
    // scenario happened to be typed into.
    assert!(d.contains("# replay: cuelight run --scenario scenario.json"), "{d}");
    // No absolute path: the same run must produce the same document wherever it is rendered.
    assert!(!d.contains("/tmp/") && !d.contains("/var/"), "a path leaked into the document:\n{d}");
    assert!(d.contains("# gst: scheduled 400"), "{d}");
    assert!(
        d.contains("in effect 700"),
        "the crash at 700 outlasts the scheduled gst, and a reader who takes the scheduled one \
         for the effective one has been caught out by exactly that:\n{d}"
    );
}
