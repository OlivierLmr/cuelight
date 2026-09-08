//! cuelight: a deterministic discrete-event simulator for distributed systems.
//!
//! The harness *is* the network. Nodes are separate processes speaking JSON lines on stdin/stdout;
//! the harness owns logical time, routes every message and injects faults. Runs replay exactly
//! from their scenario.
//!
//! It has **no notion of success**. It does not know what a property is, or what a test is: it
//! executes a scenario and writes a journal. Whoever judges that journal lives elsewhere.
//!
//! # The three public modules
//!
//! [`scenario`] builds what to run, [`sim`] runs it, [`viz`] renders what happened. A caller
//! expands a seed into a [`Scenario`], hands it to a [`Sim`] with the command that launches one
//! node, and reads the journal the run writes.
//!
//! ```no_run
//! use cuelight::{scenario::ExpandOpts, sim, Scenario};
//!
//! let sc = Scenario::expand(1, &ExpandOpts::default());
//! let outcome = sim::Sim::new(sim::Config {
//!     program: vec!["python3".into(), "node.py".into()],
//!     scenario: sc,
//!     run_dir: "store/latest".into(),
//!     watchdog: std::time::Duration::from_millis(5_000),
//! })?
//! .run()?;
//! println!("{} at t={}", outcome.reason, outcome.end_time);
//! # Ok::<(), String>(())
//! ```
//!
//! The journal format is the contract between this crate and whoever judges a run. It is specified
//! in `JOURNAL.md`, not in these types: read a run by parsing `journal.jsonl`, not by reaching for
//! the writer.

// Implementation detail, deliberately not public: the node runtimes talk to these through the wire
// protocol, and callers read runs through the journal. Exposing them would freeze internals that
// have no business being stable.
mod journal;
mod node;
mod proto;
mod rng;

pub mod scenario;
pub mod sim;
pub mod viz;

pub use scenario::{ExpandOpts, Fault, Scenario, Stimulus, StimulusSpec};
pub use sim::{Config, Outcome, Sim};
