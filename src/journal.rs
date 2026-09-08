//! Totally ordered event log: the artefact the determinism check diffs.
//!
//! Must contain nothing that varies between runs: no wall-clock, no pids, no absolute paths.
//! Logical time only.

use crate::proto::Envelope;
use serde_json::Value;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// Sort every object key, recursively.
///
/// A body reaches us in whatever order its runtime serialised it, and runtimes disagree: Go's
/// encoder sorts keys, the others keep insertion order. Ordering carries no meaning in JSON, so
/// journalling it as it arrived would make two nodes that behaved identically look different. The
/// journal is compared, by the determinism check and by anyone diffing two runs, so it is written
/// in one canonical form.
fn canonical(v: &Value) -> Value {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), canonical(&m[k]))).collect())
        }
        Value::Array(a) => Value::Array(a.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

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
            "body": canonical(&env.body),
        });
        let _ = writeln!(self.out, "{line}");
        self.count += 1;
    }

    pub fn note(&mut self, time: u64, kind: &str, detail: serde_json::Value) {
        let line = serde_json::json!({
            "seq": self.count, "t": time, "kind": kind, "detail": canonical(&detail),
        });
        let _ = writeln!(self.out, "{line}");
        self.count += 1;
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

#[cfg(test)]
mod tests {
    use super::canonical;
    use serde_json::json;

    /// Compared as text, because that is how the journal is compared.
    fn c(v: serde_json::Value) -> String {
        canonical(&v).to_string()
    }

    #[test]
    fn keys_come_out_sorted_whatever_order_they_went_in() {
        let a = c(json!({"type": "ping", "n": 1, "from": "n0"}));
        let b = c(json!({"from": "n0", "n": 1, "type": "ping"}));
        assert_eq!(a, b);
        assert_eq!(a, r#"{"from":"n0","n":1,"type":"ping"}"#);
    }

    #[test]
    fn nested_objects_are_sorted_too() {
        assert_eq!(
            c(json!({"b": {"z": 1, "a": 2}, "a": 3})),
            r#"{"a":3,"b":{"a":2,"z":1}}"#
        );
    }

    #[test]
    fn objects_inside_arrays_are_sorted_too() {
        assert_eq!(c(json!({"xs": [{"b": 1, "a": 2}]})), r#"{"xs":[{"a":2,"b":1}]}"#);
    }

    #[test]
    fn array_order_is_meaningful_and_left_alone() {
        assert_eq!(c(json!({"xs": [3, 1, 2]})), r#"{"xs":[3,1,2]}"#);
    }

    #[test]
    fn everything_that_is_not_a_container_passes_through() {
        assert_eq!(c(json!({"a": null, "b": true, "c": 1.5, "d": "s"})),
                   r#"{"a":null,"b":true,"c":1.5,"d":"s"}"#);
    }
}
