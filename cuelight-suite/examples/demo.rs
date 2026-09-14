//! A suite over fixture nodes, so the machinery in this crate is exercised end to end.
//!
//! It stands in for a real caller: two pairings, one written scenario that is expected to fail,
//! and one property. `tests/suite.rs` runs this binary against nodes that behave differently, and
//! reads what it prints.

use cuelight_suite::{Events, Kind, Pairing, Report, Scenario, Suite, Written};
use std::process::ExitCode;

static PAIRINGS: &[Pairing] = &[
    // A workload, and an environment quiet enough that every poke must be answered.
    Pairing {
        environment: "testdata/quiet.json",
        workload: Some("testdata/pokes.json"),
        seeds: 4,
    },
    // No workload at all: the faults are the whole of it, as for a failure detector.
    Pairing { environment: "testdata/crashes.json", workload: None, seeds: 4 },
];

static WRITTEN: &[Written] = &[Written {
    path: "testdata/nobody-home.json",
    expect_fail: &["every-poke-answered"],
    why: "both nodes are dead long before the pokes land, so none can be answered",
}];

fn check(ev: &Events, sc: &Scenario, r: &mut Report) {
    let asked = sc.stimuli.len();
    let answered = ev.observes.iter().filter(|(_, _, b)| b["type"] == "poked").count();
    r.add(
        "every-poke-answered",
        Kind::Liveness,
        answered == asked,
        format!("{answered} of {asked} pokes answered"),
    );
}

fn main() -> ExitCode {
    cuelight_suite::run(
        Suite { name: "demo", pairings: PAIRINGS, written: WRITTEN, check },
        env!("CARGO_MANIFEST_DIR"),
    )
}
