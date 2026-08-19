//! Totally ordered event log — the artefact the determinism check diffs.
//!
//! Must contain nothing that varies between runs: no wall-clock, no pids, no absolute paths.
//! Logical time only.

use crate::proto::Envelope;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub struct Journal {
    out: BufWriter<File>,
    count: u64,
}

impl Journal {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        Ok(Journal { out: BufWriter::new(File::create(path)?), count: 0 })
    }

    /// One line per event. `seq` is the harness's own ordering, independent of scheduling.
    pub fn record(&mut self, time: u64, kind: &str, env: &Envelope) {
        let line = serde_json::json!({
            "seq":  self.count,
            "t":    time,
            "kind": kind,
            "src":  env.src,
            "dest": env.dest,
            "body": env.body,
        });
        let _ = writeln!(self.out, "{line}");
        self.count += 1;
    }

    pub fn note(&mut self, time: u64, kind: &str, detail: serde_json::Value) {
        let line = serde_json::json!({
            "seq": self.count, "t": time, "kind": kind, "detail": detail,
        });
        let _ = writeln!(self.out, "{line}");
        self.count += 1;
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}
