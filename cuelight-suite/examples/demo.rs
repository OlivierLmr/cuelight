//! A suite over fixture nodes, so the machinery in this crate is exercised end to end.
//!
//! It stands in for a real caller: two parametric, one written scenario that is expected to fail,
//! and one property. `tests/suite.rs` runs this binary against nodes that behave differently, and
//! reads what it prints.

use cuelight_suite::{Events, Kind, Parametric, Report, Scenario, Suite, Written};
use std::process::ExitCode;

static PARAMETRIC: &[Parametric] = &[
    // Pokes, in a world quiet enough that every one of them must be answered.
    Parametric { space: "testdata/quiet.json", seeds: 4 },
    // No stimulus at all: the faults are the whole of it, as for a failure detector.
    Parametric { space: "testdata/crashes.json", seeds: 4 },
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
        Suite { name: "demo", parametric: PARAMETRIC, written: WRITTEN, check },
        env!("CARGO_MANIFEST_DIR"),
    )
}
