//! Mermaid `sequenceDiagram` output.
//!
//! Renders natively on GitHub and in most doc tools, so a run can be pasted straight into a report.
//! Cheap to produce because the journal is already totally ordered by logical time.
//!
//! What the diagram is for: seeing *why* a run went wrong. That means showing what the outside
//! world asked for, what each node reported, what the network did to them, and which messages never
//! arrived. An earlier version drew only deliveries and crashes, so deliveries appeared out of
//! nowhere, and a message its sender never got out left no trace at all.

use serde_json::Value;
use std::io::Write;
use std::path::Path;

/// Note backgrounds, by what the note *is*.
///
/// Mermaid has no per-note colour: `themeVariables` sets one colour for every note, and the
/// `themeCSS` that could target individual ones is stripped at the security level GitHub renders
/// with. What works is a one-statement `rect`, which hugs the note to within ten pixels rather than
/// banding the whole diagram — so notes are made transparent in the header below, and the rect
/// behind each one *is* its background.
/// Tints of Okabe-Ito, which stays distinguishable under every common form of colour blindness.
/// Green against red would be the one pair to avoid.
const ASKED: &str = "rect rgb(250, 228, 190)"; // orange: the world asked for something
const REPORTED: &str = "rect rgb(199, 228, 246)"; // sky blue: a node reported something
const BEFELL: &str = "rect rgb(236, 205, 224)"; // purple: the network did something to a node

/// Notes are made transparent so the coloured rect behind each one shows through, and the lifeline
/// is dimmed because it is drawn *between* the two and would otherwise cut across every label.
const HEADER: &str = concat!(
    r##"%%{init: {"themeVariables": {"noteBkgColor": "transparent", "##,
    r##""noteBorderColor": "transparent", "actorLineColor": "#d5d5d5"}}}%%"##,
    "\nsequenceDiagram\n"
);

pub fn render(journal: &Path, out: &Path, limit: usize) -> Result<usize, String> {
    let text = std::fs::read_to_string(journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let entries: Vec<Value> =
        text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect();

    // What a message spent in flight. Matched oldest-first per link, which is what FIFO means.
    let mut pending: std::collections::HashMap<(String, String), Vec<usize>> = Default::default();
    // When each delivered message was posted. A mermaid arrow is horizontal, so it cannot slant to
    // show flight time; the label carries both instants instead, and a slow link becomes visible.
    let mut posted: std::collections::HashMap<usize, u64> = Default::default();
    for (i, v) in entries.iter().enumerate() {
        let get = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        match v.get("kind").and_then(Value::as_str).unwrap_or("") {
            "send" => {
                if let (Some(s), Some(d)) = (get("src"), get("dest")) {
                    pending.entry((s, d)).or_default().push(i);
                }
            }
            "recv" => {
                if let (Some(s), Some(d)) = (get("src"), get("dest")) {
                    if let Some(q) = pending.get_mut(&(s, d)) {
                        if !q.is_empty() {
                            let j = q.remove(0);
                            if let Some(t0) = entries[j].get("t").and_then(Value::as_u64) {
                                posted.insert(i, t0);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    let f = std::fs::File::open(journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let mut lines: Vec<String> = Vec::new();
    let mut participants: Vec<String> = Vec::new();
    let mut shown = 0usize;

    // Bring a name into the diagram, in first-seen order; the caller sorts the node names later.
    fn seen(p: &str, participants: &mut Vec<String>) {
        if !participants.iter().any(|x| x == p) {
            participants.push(p.to_string());
        }
    }
    fn note(colour: &str, p: &str, text: String, lines: &mut Vec<String>) {
        lines.push(format!("    {colour}\n        Note over {p}: {text}\n    end"));
    }
    // Mermaid ends a note at a colon, and a stray one silently truncates the label.
    fn safe(s: &str) -> String {
        s.replace(':', " ")
    }

    let _ = f;
    for (i, v) in entries.iter().enumerate() {
        let v = v.clone();
        let kind = v.get("kind").and_then(Value::as_str).unwrap_or("");
        let t = v.get("t").and_then(Value::as_u64).unwrap_or(0);
        let ty = |v: &Value| {
            v.get("body")
                .and_then(|b| b.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("msg")
                .to_string()
        };
        let at = |k: &str| v.pointer(&format!("/detail/{k}")).and_then(Value::as_str).map(str::to_string);

        match kind {
            // Every process of the run, even one that never sends or receives: an empty lifeline
            // is exactly how you see that somebody was left out.
            "init" => {
                if let Some(n) = v.get("dest").and_then(Value::as_str) {
                    seen(n, &mut participants);
                }
            }
            // The world asks a node for something. This is where a run starts, and drawing only
            // what followed left every delivery without a cause.
            "stimulus" => {
                let dst = v.get("dest").and_then(Value::as_str).unwrap_or("?").to_string();
                seen(&dst, &mut participants);
                note(ASKED, &dst, format!("asked {} @{t}", safe(&ty(&v))), &mut lines);
            }
            // Delivery, not send: the diagram should show when a message actually landed.
            "recv" => {
                if shown >= limit {
                    continue;
                }
                let src = v.get("src").and_then(Value::as_str).unwrap_or("?").to_string();
                let dst = v.get("dest").and_then(Value::as_str).unwrap_or("?").to_string();
                seen(&src, &mut participants);
                seen(&dst, &mut participants);
                // Both instants, so the time a message spent in flight is readable.
                let when = match posted.get(&i) {
                    Some(t0) if *t0 != t => format!("{t0} to {t}"),
                    _ => format!("@{t}"),
                };
                lines.push(format!("    {src}->>{dst}: {} {when}", safe(&ty(&v))));
                shown += 1;
            }
            // A send its author never made: it died partway through its send loop. Worth drawing,
            // because a broadcast left half-delivered is the whole reason reliable broadcast exists.
            "never-sent" => {
                let (src, dst) = (at("src"), at("dest"));
                if let (Some(s), Some(d)) = (src, dst) {
                    seen(&s, &mut participants);
                    seen(&d, &mut participants);
                    lines.push(format!("    {s}-x{d}: never left, {s} had died @{t}"));
                }
            }
            "fault-crash" => {
                let n = at("node").unwrap_or_else(|| "?".into());
                seen(&n, &mut participants);
                note(BEFELL, &n, format!("CRASH @{t}"), &mut lines);
            }
            "fault-pause" => {
                let n = at("node").unwrap_or_else(|| "?".into());
                let until = v.pointer("/detail/until").and_then(Value::as_u64).unwrap_or(t);
                seen(&n, &mut participants);
                // "pause" said nothing about what stops. A paused node handles no event until it
                // wakes; its clock keeps running and its messages queue up.
                note(BEFELL, &n, format!("frozen, handles nothing until @{until}"), &mut lines);
            }
            // A partition is an event of the network, not of one node. Noting it on a single node
            // said nothing about who was cut off from whom, and the node it landed on was whichever
            // happened to be seen first, sometimes one that had already crashed.
            "fault-partition" => {
                let side: Vec<String> = v
                    .pointer("/detail/side")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                let until = v.pointer("/detail/until").and_then(Value::as_u64).unwrap_or(t);
                let mut everyone: Vec<String> = participants.clone();
                for s in &side {
                    if !everyone.contains(s) {
                        everyone.push(s.clone());
                    }
                }
                everyone.sort();
                for n in &everyone {
                    let group = if side.contains(n) { "A" } else { "B" };
                    seen(n, &mut participants);
                    // Short on purpose: one note per node means this line repeats once per node,
                    // and a sentence there buries the rest of the run.
                    note(BEFELL, n, format!("SPLIT {group} until @{until}"), &mut lines);
                }
            }
            "observe" => {
                if shown >= limit {
                    continue;
                }
                let src = v.get("src").and_then(Value::as_str).unwrap_or("?").to_string();
                seen(&src, &mut participants);
                note(REPORTED, &src, format!("{} @{t}", safe(&ty(&v))), &mut lines);
            }
            _ => {}
        }
    }

    // Only processes appear. The harness has no lifeline of its own: what it asks for is a note
    // on the node it asks, and a legend belongs in the documentation rather than in every diagram.
    let mut names: Vec<String> = participants.clone();
    names.sort();

    let mut doc = String::from(HEADER);
    for p in &names {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Render a journal given as lines, and hand back the diagram.
    ///
    /// `name` keeps each test in its own directory: tests run in parallel, and naming the
    /// directory after anything they share makes them clobber each other's files.
    fn draw(name: &str, lines: &[&str]) -> String {
        let dir = std::env::temp_dir().join(format!("cuelight-viz-{name}"));
        let _ = std::fs::create_dir_all(&dir);
        let j = dir.join("journal.jsonl");
        let mut f = std::fs::File::create(&j).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        drop(f);
        let out = dir.join("m.mmd");
        render(&j, &out, 100).unwrap();
        std::fs::read_to_string(out).unwrap()
    }

    /// A run starts because the world asked for something. Drawing only what followed left every
    /// delivery without a cause. The harness gets no lifeline of its own: it is not a process, and
    /// a column for it is a column of nothing.
    #[test]
    fn a_stimulus_is_a_note_on_the_node_it_was_asked_of() {
        let d = draw("stimulus", &[
            r#"{"t":10,"kind":"stimulus","src":"harness","dest":"n0","body":{"type":"shout"}}"#,
        ]);
        assert!(!d.contains("participant harness"), "the harness is not a process: {d}");
        assert!(d.contains(&format!("{ASKED}\n        Note over n0: asked shout @10")), "{d}");
    }

    /// Every process of the run, even one that never sends or receives. An empty lifeline is
    /// exactly how a reader sees that somebody was left out.
    #[test]
    fn every_process_gets_a_lifeline_even_a_silent_one() {
        let d = draw("silent", &[
            r#"{"t":0,"kind":"init","src":"harness","dest":"n0","body":{"type":"init"}}"#,
            r#"{"t":0,"kind":"init","src":"harness","dest":"n1","body":{"type":"init"}}"#,
            r#"{"t":0,"kind":"init","src":"harness","dest":"n2","body":{"type":"init"}}"#,
            r#"{"t":5,"kind":"recv","src":"n0","dest":"n1","body":{"type":"x"}}"#,
        ]);
        for n in ["n0", "n1", "n2"] {
            assert!(d.contains(&format!("participant {n}")), "{n} missing: {d}");
        }
    }

    /// A mermaid arrow is horizontal and cannot slant, so flight time has to be in the label.
    #[test]
    fn a_message_carries_when_it_left_and_when_it_landed() {
        let d = draw("flight", &[
            r#"{"t":10,"kind":"send","src":"n0","dest":"n1","body":{"type":"rb"}}"#,
            r#"{"t":80,"kind":"recv","src":"n0","dest":"n1","body":{"type":"rb"}}"#,
        ]);
        assert!(d.contains("n0->>n1: rb 10 to 80"), "{d}");
    }

    /// A send its author never made is the interesting half of a crash, and it used to leave no
    /// trace at all.
    #[test]
    fn a_send_that_never_left_is_a_crossed_arrow_that_says_why() {
        let d = draw("lost", &[
            r#"{"t":20,"kind":"never-sent","detail":{"src":"n0","dest":"n1"}}"#,
        ]);
        assert!(d.contains("n0-xn1: never left, n0 had died @20"), "{d}");
    }

    /// A partition is an event of the network. It used to be noted on whichever node happened to
    /// be seen first, sometimes one that had already crashed, and said nothing about the sides.
    #[test]
    fn a_partition_tells_every_node_which_side_it_is_on() {
        let d = draw("split", &[
            r#"{"t":1,"kind":"recv","src":"n0","dest":"n1","body":{"type":"x"}}"#,
            r#"{"t":2,"kind":"recv","src":"n2","dest":"n3","body":{"type":"x"}}"#,
            r#"{"t":40,"kind":"fault-partition","detail":{"side":["n0","n1"],"until":90}}"#,
        ]);
        for n in ["n0", "n1"] {
            assert!(d.contains(&format!("Note over {n}: SPLIT A")), "{n} missing in {d}");
        }
        for n in ["n2", "n3"] {
            assert!(d.contains(&format!("Note over {n}: SPLIT B")), "{n} missing in {d}");
        }
        assert!(d.contains("until @90"), "{d}");
    }

    #[test]
    fn the_harness_is_on_no_side_of_a_split() {
        let d = draw("split-harness", &[
            r#"{"t":1,"kind":"stimulus","src":"harness","dest":"n0","body":{"type":"go"}}"#,
            r#"{"t":40,"kind":"fault-partition","detail":{"side":["n0"],"until":90}}"#,
        ]);
        assert!(!d.contains("harness"), "the harness is not a process: {d}");
    }

    /// "pause" named the cause, not the effect. What a reader needs is what stops, and until when.
    #[test]
    fn a_pause_says_what_stops_and_until_when() {
        let d = draw("pause", &[r#"{"t":5,"kind":"fault-pause","detail":{"node":"n1","until":300}}"#]);
        assert!(d.contains("Note over n1: frozen, handles nothing until @300"), "{d}");
    }

    #[test]
    fn every_note_carries_the_colour_of_what_it_is() {
        let d = draw("colours", &[
            r#"{"t":1,"kind":"stimulus","src":"harness","dest":"n0","body":{"type":"go"}}"#,
            r#"{"t":2,"kind":"observe","src":"n0","body":{"type":"deliver"}}"#,
            r#"{"t":3,"kind":"fault-crash","detail":{"node":"n0"}}"#,
        ]);
        assert!(d.contains(&format!("{ASKED}\n        Note over n0: asked go @1")), "{d}");
        assert!(d.contains(&format!("{REPORTED}\n        Note over n0: deliver @2")), "{d}");
        assert!(d.contains(&format!("{BEFELL}\n        Note over n0: CRASH @3")), "{d}");
        // Notes are transparent so the rect behind each one is what carries the colour.
        assert!(d.contains(r#""noteBkgColor": "transparent""#), "{d}");
    }

    #[test]
    fn participants_are_listed_in_name_order() {
        let d = draw("order", &[
            r#"{"t":1,"kind":"recv","src":"n9","dest":"n2","body":{"type":"x"}}"#,
            r#"{"t":2,"kind":"stimulus","src":"harness","dest":"n9","body":{"type":"go"}}"#,
        ]);
        let order: Vec<&str> =
            d.lines().filter_map(|l| l.trim().strip_prefix("participant ")).collect();
        assert_eq!(order, vec!["n2", "n9"], "{d}");
    }

    /// Mermaid ends a note at a colon, so one inside a label silently truncates it.
    #[test]
    fn a_colon_in_a_label_does_not_truncate_it() {
        let d = draw("colon", &[r#"{"t":1,"kind":"observe","src":"n0","body":{"type":"a:b"}}"#]);
        assert!(d.contains("Note over n0: a b @1"), "{d}");
    }
}
