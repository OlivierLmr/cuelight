//! SDR harness — deterministic discrete-event simulator for the distributed systems labs.
//!
//! The harness *is* the network. Student nodes are separate processes speaking JSON lines on
//! stdin/stdout; the harness owns logical time, routes every message, injects faults, and checks
//! properties. Runs are fully deterministic: a scenario replays exactly.
//!
//! Status: skeleton. See `docs/implementation-plan.md`, milestone M0.

fn main() {
    eprintln!("sdr-harness 0.1.0 — skeleton, see docs/implementation-plan.md (M0)");
    std::process::exit(1);
}
