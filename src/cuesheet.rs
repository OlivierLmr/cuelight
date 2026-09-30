//! A journal, as a cuesheet document.
//!
//! The other half of the split this repository already draws: cuelight executes a scenario and
//! writes down what happened; something else decides what that is worth looking at. So this
//! emitter knows nothing about what a message *means*. It writes the protocol's own words into the
//! kind slot and leaves a style sheet to say what they are — which is why there is no list of
//! message types here, and must never be one.
//!
//! No dependency on cuesheet, either. The document is text, and the arrow is the whole syntax.

use serde_json::Value;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;

/// Quote a value only when the bare form would not survive being read back.
///
/// The same rule the reader uses, so a hand-written `rb` and a generated one are the same bytes.
/// Never "quote to be safe": that would make every generated document differ from the obvious
/// hand-written spelling of the same thing.
fn word(s: &str) -> String {
    let bare = |c: char| {
        c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | ':' | '/' | '-' | '=' | '#' | ',')
    };
    let needs = s.is_empty()
        || !s.chars().all(bare)
        || s.starts_with('#')
        || s.starts_with('.')
        || s.starts_with('+')
        || s.starts_with('@');
    if !needs {
        return s.to_string();
    }
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn str_at<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}
fn u64_at(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}
fn detail<'a>(v: &'a Value, k: &str) -> Option<&'a Value> {
    v.get("detail").and_then(|d| d.get(k))
}

/// The body's `type`, which is the protocol's own word for this message.
fn body_type(v: &Value) -> String {
    v.get("body")
        .and_then(|b| b.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("message")
        .to_string()
}

/// One line of the document, with the instant it happens at so the whole set can be ordered once.
struct Line {
    t: u64,
    seq: u64,
    text: String,
}

pub fn render(
    journal: &Path,
    scenario: Option<&Path>,
    out: &Path,
    limit: usize,
) -> Result<usize, String> {
    let text =
        std::fs::read_to_string(journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let entries: Vec<Value> =
        text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect();

    let mut participants: Vec<String> = Vec::new();
    let seen = |n: &str, ps: &mut Vec<String>| {
        if !n.is_empty() && n != crate::proto::HARNESS && !ps.iter().any(|p| p == n) {
            ps.push(n.to_string());
        }
    };

    // Where each message landed, by the name the harness gave it. Pairing on `mid` rather than
    // oldest-first per link is the whole reason the field exists: a link that reorders would
    // otherwise pair a `recv` with the wrong `send`, and a space-time diagram draws that mistake as
    // two arrows crossing that never crossed.
    let mut arrival: BTreeMap<u64, u64> = BTreeMap::new();
    for e in &entries {
        let kind = str_at(e, "kind").unwrap_or("");
        if kind == "recv" {
            if let (Some(mid), Some(t)) = (u64_at(e, "mid"), u64_at(e, "t")) {
                arrival.insert(mid, t);
            }
        }
        if kind == "drop-to-crashed" {
            // It reached the lifeline and died there, which is an arrival as far as the drawing is
            // concerned — the renderer works out the rest from the crash.
            if let (Some(mid), Some(t)) =
                (detail(e, "mid").and_then(Value::as_u64), u64_at(e, "t"))
            {
                arrival.insert(mid, t);
            }
        }
    }

    // Timers: `set_timer` opens and `timer` closes, paired per node and id, oldest first.
    let mut timers: BTreeMap<(String, u64), Vec<u64>> = BTreeMap::new();
    for e in &entries {
        if str_at(e, "kind") == Some("timer") {
            if let (Some(n), Some(id), Some(t)) = (
                str_at(e, "dest"),
                e.get("body").and_then(|b| b.get("timer_id")).and_then(Value::as_u64),
                u64_at(e, "t"),
            ) {
                timers.entry((n.to_string(), id)).or_default().push(t);
            }
        }
    }

    let mut lines: Vec<Line> = Vec::new();
    let mut drawn = 0usize;
    let mut end: Option<Line> = None;

    for e in &entries {
        let kind = str_at(e, "kind").unwrap_or("");
        let t = u64_at(e, "t").unwrap_or(0);
        let seq = u64_at(e, "seq").unwrap_or(0);
        let mut push = |text: String| lines.push(Line { t, seq, text });

        match kind {
            // Every process of the run, even one that never speaks. An empty lifeline is exactly
            // how a reader sees that somebody was left out.
            "init" => {
                if let Some(n) = str_at(e, "dest") {
                    seen(n, &mut participants);
                }
            }

            // The world asked a node for something. Written with the protocol's own word as the
            // kind and `.stimulus` as the class, so a style sheet can tell it from an observation
            // of the same name without the harness having to invent a vocabulary.
            "stimulus" => {
                if let Some(n) = str_at(e, "dest") {
                    seen(n, &mut participants);
                    push(format!("{t} {} {} .stimulus", word(n), word(&body_type(e))));
                }
            }

            "observe" => {
                if let Some(n) = str_at(e, "src") {
                    seen(n, &mut participants);
                    push(format!("{t} {} {} .observe", word(n), word(&body_type(e))));
                }
            }

            "send" => {
                if drawn >= limit {
                    continue;
                }
                let (src, dest) = (str_at(e, "src"), str_at(e, "dest"));
                if let (Some(src), Some(dest)) = (src, dest) {
                    seen(src, &mut participants);
                    seen(dest, &mut participants);
                    let ty = body_type(e);
                    match u64_at(e, "mid").and_then(|m| arrival.get(&m)) {
                        Some(at) => push(format!(
                            "{t} {} -> {} .{} @{at}",
                            word(src),
                            word(dest),
                            word(&ty)
                        )),
                        // It left and never landed: the run ended first, or a partition was still
                        // holding it. Either way the document says when it left and where it was
                        // going, and the renderer works out that it is still in flight.
                        None => push(format!(
                            "{t} {} -> {} .{} @{}",
                            word(src),
                            word(dest),
                            word(&ty),
                            u64::MAX
                        )),
                    }
                    drawn += 1;
                }
            }

            // A send its author never made: it died partway through its send loop. Written as an
            // ordinary arrow, because the crash is already in the document and the renderer derives
            // the rest. `+1` because the journal never learned where it would have landed, and the
            // instant is only used for the direction of the stub.
            "never-sent" => {
                let (src, dest) = (
                    detail(e, "src").and_then(Value::as_str),
                    detail(e, "dest").and_then(Value::as_str),
                );
                if let (Some(src), Some(dest)) = (src, dest) {
                    seen(src, &mut participants);
                    seen(dest, &mut participants);
                    push(format!("{t} {} -> {} +1", word(src), word(dest)));
                }
            }

            "fault-crash" => {
                if let Some(n) = detail(e, "node").and_then(Value::as_str) {
                    seen(n, &mut participants);
                    push(format!("{t} {} crash", word(n)));
                }
            }

            "node-died" => {
                if let Some(n) = detail(e, "node").and_then(Value::as_str) {
                    seen(n, &mut participants);
                    push(format!("{t} {} died", word(n)));
                }
            }

            // Alive, handling nothing. A span rather than a point, because it lasts.
            "fault-pause" => {
                if let Some(n) = detail(e, "node").and_then(Value::as_str) {
                    let until = detail(e, "until").and_then(Value::as_u64).unwrap_or(t);
                    seen(n, &mut participants);
                    push(format!("{t} {} paused @{until}", word(n)));
                }
            }

            // A partition is an event of the network, and it names the lanes on one side. Shading
            // named lanes rather than drawing a dividing line is what makes it expressible at all:
            // the two sides are not generally next to each other.
            "fault-partition" => {
                let side: Vec<String> = detail(e, "side")
                    .and_then(Value::as_array)
                    .map(|a| a.iter().filter_map(Value::as_str).map(word).collect())
                    .unwrap_or_default();
                let until = detail(e, "until").and_then(Value::as_u64).unwrap_or(t);
                if !side.is_empty() {
                    push(format!("{t} network split {} @{until}", side.join(" ")));
                }
            }

            "set_timer" => {
                if let (Some(n), Some(id)) = (
                    str_at(e, "src"),
                    e.get("body").and_then(|b| b.get("timer_id")).and_then(Value::as_u64),
                ) {
                    seen(n, &mut participants);
                    // Paired with its firing, oldest first, so the pair draws as one span.
                    let fired = timers.get_mut(&(n.to_string(), id)).and_then(|v| {
                        if v.is_empty() {
                            None
                        } else {
                            Some(v.remove(0))
                        }
                    });
                    match fired {
                        Some(at) => push(format!("{t} {} set_timer @{at}", word(n))),
                        // Armed and never fired: the node crashed, or the run ended first.
                        None => push(format!("{t} {} set_timer", word(n))),
                    }
                }
            }

            "unknown-destination" => {
                if let Some(d) = detail(e, "dest").and_then(Value::as_str) {
                    push(format!("{t} network unknown-destination {}", word(d)));
                }
            }

            "time-limit" | "end" => {
                let reason = detail(e, "reason")
                    .and_then(Value::as_str)
                    .unwrap_or(if kind == "time-limit" { "time-limit" } else { "end" });
                end = Some(Line { t, seq, text: format!("{t} run end {}", word(reason)) });
            }

            _ => {}
        }
    }

    // A message that never landed was written with a placeholder arrival; now that the end of the
    // run is known, say it plainly.
    let stop = end.as_ref().map(|l| l.t).unwrap_or_else(|| lines.iter().map(|l| l.t).max().unwrap_or(0));
    for l in &mut lines {
        if l.text.ends_with(&format!("@{}", u64::MAX)) {
            l.text = l.text.replace(&format!("@{}", u64::MAX), &format!("@{}", stop + 1));
        }
    }
    if let Some(e) = end {
        lines.push(e);
    }

    // One order, always: by instant, then by the harness's own sequence. Two runs of the same
    // scenario produce the same bytes, which is what `check` rests on and what makes a regenerated
    // diagram diff against the one before it.
    lines.sort_by_key(|l| (l.t, l.seq));
    participants.sort();

    let mut doc = String::new();
    if let Some(s) = scenario {
        doc.push_str(&provenance(s));
    }
    doc.push_str(&format!("participants {}\n\n", participants.join(" ")));
    for l in &lines {
        doc.push_str(&l.text);
        doc.push('\n');
    }

    let mut w = std::fs::File::create(out).map_err(|e| format!("{}: {e}", out.display()))?;
    w.write_all(doc.as_bytes()).map_err(|e| e.to_string())?;
    Ok(drawn)
}

/// Which run this came from, as comments nothing reads back.
///
/// cuelight is the only party that knows the seed, the space and the fingerprint, because it is the
/// only one that computed them. A figure in a handout can then name the run behind it.
fn provenance(scenario: &Path) -> String {
    let Ok(text) = std::fs::read_to_string(scenario) else {
        return String::new();
    };
    let Ok(v) = serde_json::from_str::<Value>(&text) else {
        return String::new();
    };
    let mut out = String::new();
    if let Some(d) = v.get("drawn") {
        let seed = u64_at(d, "seed").map(|s| s.to_string()).unwrap_or_else(|| "?".into());
        let space = str_at(d, "space").unwrap_or("?");
        let fp = str_at(d, "fingerprint").unwrap_or("?");
        out.push_str(&format!("# drawn: seed {seed} · {space} · {fp}\n"));
    }
    // The file's name, never its path. An absolute path would make the same run produce a
    // different document depending on which directory it was rendered from — the same rule the
    // journal already holds itself to, and for the same reason.
    let named = scenario
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("scenario.json");
    out.push_str(&format!("# replay: cuelight run --scenario {named} --bin <your node>\n"));

    // GST twice when the two differ, because the scheduled one is not the effective one and a
    // reader who takes it for the effective one has been caught out by exactly that before.
    if let Some(gst) = u64_at(&v, "gst") {
        let last_fault = v
            .get("faults")
            .and_then(Value::as_array)
            .map(|fs| {
                fs.iter()
                    .filter_map(|f| {
                        let at = u64_at(f, "at")?;
                        Some(at + u64_at(f, "duration").unwrap_or(0))
                    })
                    .max()
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        out.push_str(&format!("# gst: scheduled {gst}"));
        if last_fault > gst {
            out.push_str(&format!(", in effect {last_fault} (the last fault ends there)"));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Write a journal and render it, handing back the document.
    fn draw(name: &str, lines: &[&str]) -> String {
        let dir = std::env::temp_dir().join(format!("cuelight-cuesheet-{name}"));
        let _ = std::fs::create_dir_all(&dir);
        let j = dir.join("journal.jsonl");
        let mut f = std::fs::File::create(&j).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        drop(f);
        let out = dir.join("messages.cuesheet");
        render(&j, None, &out, 1000).unwrap();
        std::fs::read_to_string(out).unwrap()
    }

    #[test]
    fn every_process_gets_a_lane_even_a_silent_one() {
        let d = draw("silent", &[
            r#"{"seq":0,"t":0,"kind":"init","src":"harness","dest":"n0","body":{"type":"init"}}"#,
            r#"{"seq":1,"t":0,"kind":"init","src":"harness","dest":"n1","body":{"type":"init"}}"#,
            r#"{"seq":2,"t":0,"kind":"init","src":"harness","dest":"n2","body":{"type":"init"}}"#,
        ]);
        assert!(d.contains("participants n0 n1 n2"), "{d}");
    }

    /// The harness is not a process, so it gets no lane.
    #[test]
    fn the_harness_never_becomes_a_participant() {
        let d = draw("noharness", &[
            r#"{"seq":0,"t":1,"kind":"stimulus","src":"harness","dest":"n0","body":{"type":"go"}}"#,
        ]);
        assert!(!d.contains("harness"), "{d}");
        assert!(d.contains("participants n0"), "{d}");
    }

    /// The arrow carries both instants, so its slope is its flight time.
    #[test]
    fn a_message_is_an_arrow_from_when_it_left_to_when_it_landed() {
        let d = draw("flight", &[
            r#"{"seq":0,"t":10,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"rb"}}"#,
            r#"{"seq":1,"t":80,"kind":"recv","src":"n0","dest":"n1","mid":1,"body":{"type":"rb"}}"#,
        ]);
        assert!(d.contains("10 n0 -> n1 .rb @80"), "{d}");
    }

    /// The whole reason `mid` exists: a link that reorders would otherwise pair a `recv` with the
    /// wrong `send`, and a space-time diagram draws that as two arrows crossing that never crossed.
    #[test]
    fn messages_pair_by_name_rather_than_by_arrival_order() {
        let d = draw("reorder", &[
            r#"{"seq":0,"t":10,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"a"}}"#,
            r#"{"seq":1,"t":20,"kind":"send","src":"n0","dest":"n1","mid":2,"body":{"type":"b"}}"#,
            // b overtakes a on the wire.
            r#"{"seq":2,"t":50,"kind":"recv","src":"n0","dest":"n1","mid":2,"body":{"type":"b"}}"#,
            r#"{"seq":3,"t":90,"kind":"recv","src":"n0","dest":"n1","mid":1,"body":{"type":"a"}}"#,
        ]);
        assert!(d.contains("10 n0 -> n1 .a @90"), "a should land at 90: {d}");
        assert!(d.contains("20 n0 -> n1 .b @50"), "b should land at 50: {d}");
    }

    /// A message that arrives at a dead node lands on the lifeline; the renderer works out the rest
    /// from the crash, which is also in the document.
    #[test]
    fn a_message_that_died_at_a_lifeline_still_says_where_it_was_going() {
        let d = draw("died", &[
            r#"{"seq":0,"t":10,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"rb"}}"#,
            r#"{"seq":1,"t":20,"kind":"fault-crash","detail":{"node":"n1"}}"#,
            r#"{"seq":2,"t":30,"kind":"drop-to-crashed","detail":{"src":"n0","dest":"n1","mid":1}}"#,
        ]);
        assert!(d.contains("10 n0 -> n1 .rb @30"), "{d}");
        assert!(d.contains("20 n1 crash"), "{d}");
    }

    #[test]
    fn a_send_its_author_never_made_is_written_as_an_ordinary_arrow() {
        let d = draw("neversent", &[
            r#"{"seq":0,"t":10,"kind":"fault-crash","detail":{"node":"n0"}}"#,
            r#"{"seq":1,"t":20,"kind":"never-sent","detail":{"src":"n0","dest":"n1","mid":3}}"#,
        ]);
        assert!(d.contains("10 n0 crash"), "{d}");
        assert!(d.contains("20 n0 -> n1 +1"), "{d}");
    }

    #[test]
    fn a_message_still_in_flight_when_the_run_ends_says_so_by_landing_after_it() {
        let d = draw("inflight", &[
            r#"{"seq":0,"t":10,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"rb"}}"#,
            r#"{"seq":1,"t":50,"kind":"end","detail":{"reason":"quiescent"}}"#,
        ]);
        assert!(d.contains("10 n0 -> n1 .rb @51"), "{d}");
        assert!(d.contains("50 run end quiescent"), "{d}");
    }

    /// The protocol's own words go in the kind slot; the journal's categorisation goes in a class.
    /// A stimulus and an observation of the same name have to be tellable apart without the harness
    /// inventing a vocabulary.
    #[test]
    fn a_stimulus_and_an_observation_are_told_apart_by_class_not_by_name() {
        let d = draw("classes", &[
            r#"{"seq":0,"t":1,"kind":"stimulus","src":"harness","dest":"n0","body":{"type":"go"}}"#,
            r#"{"seq":1,"t":2,"kind":"observe","src":"n0","body":{"type":"go"}}"#,
        ]);
        assert!(d.contains("1 n0 go .stimulus"), "{d}");
        assert!(d.contains("2 n0 go .observe"), "{d}");
    }

    #[test]
    fn a_pause_is_a_span_that_ends_when_the_node_wakes() {
        let d = draw("pause", &[
            r#"{"seq":0,"t":5,"kind":"fault-pause","detail":{"node":"n1","until":300}}"#,
        ]);
        assert!(d.contains("5 n1 paused @300"), "{d}");
    }

    /// The two sides of a split are not generally next to each other, so the document names the
    /// lanes rather than describing a line between them.
    #[test]
    fn a_partition_names_the_lanes_on_one_side() {
        let d = draw("split", &[
            r#"{"seq":0,"t":40,"kind":"fault-partition","detail":{"side":["n0","n2"],"until":90}}"#,
        ]);
        assert!(d.contains("40 network split n0 n2 @90"), "{d}");
    }

    #[test]
    fn a_timer_is_a_span_from_arming_to_firing() {
        let d = draw("timer", &[
            r#"{"seq":0,"t":10,"kind":"set_timer","src":"n0","dest":"harness","body":{"after":50,"timer_id":1}}"#,
            r#"{"seq":1,"t":60,"kind":"timer","src":"harness","dest":"n0","body":{"timer_id":1,"type":"timer"}}"#,
        ]);
        assert!(d.contains("10 n0 set_timer @60"), "{d}");
    }

    #[test]
    fn a_timer_that_never_fires_is_left_open() {
        let d = draw("timer-open", &[
            r#"{"seq":0,"t":10,"kind":"set_timer","src":"n0","dest":"harness","body":{"after":50,"timer_id":1}}"#,
            r#"{"seq":1,"t":20,"kind":"fault-crash","detail":{"node":"n0"}}"#,
        ]);
        assert!(d.contains("10 n0 set_timer\n"), "{d}");
    }

    #[test]
    fn a_node_that_exited_on_its_own_is_not_written_as_a_crash() {
        let d = draw("died-own", &[r#"{"seq":0,"t":7,"kind":"node-died","detail":{"node":"n2"}}"#]);
        assert!(d.contains("7 n2 died"), "{d}");
        assert!(!d.contains("crash"), "{d}");
    }

    /// One order, always, so two runs of the same scenario produce the same bytes.
    #[test]
    fn lines_come_out_in_time_order_then_sequence_order() {
        let d = draw("order", &[
            r#"{"seq":0,"t":9,"kind":"observe","src":"n1","body":{"type":"late"}}"#,
            r#"{"seq":1,"t":1,"kind":"observe","src":"n0","body":{"type":"early"}}"#,
        ]);
        let early = d.find("early").unwrap();
        let late = d.find("late").unwrap();
        assert!(early < late, "{d}");
    }

    #[test]
    fn a_type_that_is_not_a_bare_word_is_quoted() {
        let d = draw("quoting", &[
            r#"{"seq":0,"t":1,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"rb round 2"}}"#,
            r#"{"seq":1,"t":2,"kind":"recv","src":"n0","dest":"n1","mid":1,"body":{"type":"rb round 2"}}"#,
        ]);
        assert!(d.contains(r#".."rb round 2""#) || d.contains(r#"."rb round 2""#), "{d}");
    }

    /// A colon survives bare, because a message type is an arbitrary JSON string and the reader's
    /// bare-word set has colons in it for exactly this reason.
    #[test]
    fn a_colon_in_a_type_needs_no_quoting() {
        let d = draw("colon", &[
            r#"{"seq":0,"t":1,"kind":"observe","src":"n0","body":{"type":"a:b"}}"#,
        ]);
        assert!(d.contains("1 n0 a:b .observe"), "{d}");
    }

    #[test]
    fn the_same_journal_always_produces_the_same_document() {
        let j = &[
            r#"{"seq":0,"t":10,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"rb"}}"#,
            r#"{"seq":1,"t":80,"kind":"recv","src":"n0","dest":"n1","mid":1,"body":{"type":"rb"}}"#,
            r#"{"seq":2,"t":90,"kind":"end","detail":{"reason":"quiescent"}}"#,
        ];
        assert_eq!(draw("det-a", j), draw("det-b", j));
    }
}
