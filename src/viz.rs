//! Mermaid `sequenceDiagram` output.
//!
//! Renders natively on GitHub and in most doc tools, so students can paste a run straight into a
//! report. Cheap to produce because the journal is already totally ordered by logical time.

use serde_json::Value;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

pub fn render(journal: &Path, out: &Path, limit: usize) -> Result<usize, String> {
    let f = std::fs::File::open(journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let mut lines = Vec::new();
    let mut participants: Vec<String> = Vec::new();
    let mut shown = 0usize;

    for l in BufReader::new(f).lines() {
        let l = l.map_err(|e| e.to_string())?;
        let v: Value = match serde_json::from_str(&l) {
            Ok(v) => v,
            Err(_) => continue,
        };
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        let t = v.get("t").and_then(Value::as_u64).unwrap_or(0);

        let mut note = |p: &str, text: String, participants: &mut Vec<String>| {
            if !participants.iter().any(|x| x == p) {
                participants.push(p.to_string());
            }
            lines.push(format!("    Note over {p}: {text}"));
        };

        match kind {
            // Delivery, not send: the diagram should show when a message actually landed.
            "recv" => {
                if shown >= limit {
                    continue;
                }
                let src = v.get("src").and_then(Value::as_str).unwrap_or("?").to_string();
                let dst = v.get("dest").and_then(Value::as_str).unwrap_or("?").to_string();
                let ty = v
                    .get("body")
                    .and_then(|b| b.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("msg");
                for p in [&src, &dst] {
                    if !participants.iter().any(|x| x == p) {
                        participants.push(p.clone());
                    }
                }
                lines.push(format!("    {src}->>{dst}: {ty} @{t}"));
                shown += 1;
            }
            "fault-crash" => {
                let n = v.pointer("/detail/node").and_then(Value::as_str).unwrap_or("?").to_string();
                note(&n, format!("CRASH @{t}"), &mut participants);
            }
            "fault-pause" => {
                let n = v.pointer("/detail/node").and_then(Value::as_str).unwrap_or("?").to_string();
                note(&n, format!("pause @{t}"), &mut participants);
            }
            "fault-partition" => {
                lines.push(format!("    Note over {}: PARTITION @{t}",
                    participants.first().cloned().unwrap_or_else(|| "n0".into())));
            }
            "observe" => {
                if shown >= limit {
                    continue;
                }
                let src = v.get("src").and_then(Value::as_str).unwrap_or("?").to_string();
                let ty = v
                    .get("body")
                    .and_then(|b| b.get("type"))
                    .and_then(Value::as_str)
                    .unwrap_or("event");
                note(&src, format!("{ty} @{t}"), &mut participants);
            }
            _ => {}
        }
    }

    participants.sort();
    let mut doc = String::from("sequenceDiagram\n");
    for p in &participants {
        doc.push_str(&format!("    participant {p}\n"));
    }
    for l in &lines {
        doc.push_str(l);
        doc.push('\n');
    }

    let mut w = std::fs::File::create(out).map_err(|e| e.to_string())?;
    w.write_all(doc.as_bytes()).map_err(|e| e.to_string())?;
    Ok(shown)
}
