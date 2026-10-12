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

/// How many messages a Mermaid diagram draws before it stops.
///
/// Mermaid renderers refuse a diagram past a certain size, GitHub's among them, and a refused
/// diagram shows nothing at all. So this stays small, and the drawing says where it stopped.
pub const ARROWS: usize = 200;

///
/// When there are more than `limit`, which ones are drawn is decided by `around`: the instant the
/// reader is looking for, when something knows it — a check that failed says when. A quarter of
/// the room goes to what followed that instant and the rest to what led up to it, because a
/// failure is explained by what came before. With no instant, the drawing starts where the run
/// does.
pub fn render(journal: &Path, out: &Path, limit: usize, around: Option<u64>) -> Result<usize, String> {
    let text = std::fs::read_to_string(journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let entries: Vec<Value> =
        text.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()).collect();

    // When each delivered message was posted. A mermaid arrow is horizontal, so it cannot slant to
    // show flight time; the label carries both instants instead, and a slow link becomes visible.
    //
    // Paired on `mid`, the name the harness gives a message when it leaves its sender. This used to
    // match oldest-first per link, which is exactly right when the link is FIFO and wrong
    // otherwise — and `fifo` is a scenario field meant to be turned off, with jitter on by default,
    // so reordering genuinely happens. A mispairing put the wrong two instants on a label.
    let mut sent_at: std::collections::HashMap<u64, u64> = Default::default();
    let mut posted: std::collections::HashMap<usize, u64> = Default::default();
    for (i, v) in entries.iter().enumerate() {
        let mid = v.get("mid").and_then(Value::as_u64);
        match (v.get("kind").and_then(Value::as_str).unwrap_or(""), mid) {
            ("send", Some(m)) => {
                if let Some(t) = v.get("t").and_then(Value::as_u64) {
                    sent_at.insert(m, t);
                }
            }
            ("recv", Some(m)) => {
                if let Some(t0) = sent_at.get(&m) {
                    posted.insert(i, *t0);
                }
            }
            _ => {}
        }
    }

    // Which messages are drawn: `limit` of them in a row, counted in the order they landed.
    let landings: Vec<u64> = entries
        .iter()
        .filter(|v| v.get("kind").and_then(Value::as_str) == Some("recv"))
        .map(|v| v.get("t").and_then(Value::as_u64).unwrap_or(0))
        .collect();
    let landed = landings.len();
    let first = match around {
        Some(a) if landed > limit => {
            let before = landings.partition_point(|t| *t <= a);
            before.saturating_sub(limit - limit / 4).min(landed - limit)
        }
        _ => 0,
    };
    // Nothing is drawn before this instant but what the network did to a process, which is rare
    // and explains a lifeline that stays silent for the whole of the part that is drawn.
    let opens_at = if first > 0 { landings[first] } else { 0 };

    let mut lines: Vec<String> = Vec::new();
    let mut participants: Vec<String> = Vec::new();
    let mut shown = 0usize;
    let mut nth = 0usize;
    // Where the note goes that says the drawing does not start where the run does.
    let mut opening: Option<usize> = None;

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
    // The type, then what the event carries: `leader id=n3`. The type alone says that a process
    // reported a leader, never whom. `;` and `#` end a Mermaid statement or open an entity.
    fn told(v: &Value, ty: &str) -> String {
        let extra = crate::cuesheet::carried(v.get("body"), 40).replace([';', '#'], " ");
        if extra.is_empty() {
            safe(ty)
        } else {
            safe(&format!("{ty} {extra}"))
        }
    }

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
                if t < opens_at {
                    continue;
                }
                let dst = v.get("dest").and_then(Value::as_str).unwrap_or("?").to_string();
                seen(&dst, &mut participants);
                note(ASKED, &dst, format!("asked {} @{t}", told(&v, &ty(&v))), &mut lines);
            }
            // Delivery, not send: the diagram should show when a message actually landed.
            "recv" => {
                nth += 1;
                if nth <= first || shown >= limit {
                    continue;
                }
                if first > 0 && opening.is_none() {
                    opening = Some(lines.len());
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
                lines.push(format!("    {src}->>{dst}: {} {when}", told(&v, &ty(&v))));
                shown += 1;
            }
            // A send its author never made: it died partway through its send loop. Worth drawing,
            // because a broadcast left half-delivered is the whole reason reliable broadcast exists.
            "never-sent" => {
                let (src, dst) = (at("src"), at("dest"));
                if t < opens_at {
                    continue;
                }
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
                if t < opens_at || shown >= limit {
                    continue;
                }
                let src = v.get("src").and_then(Value::as_str).unwrap_or("?").to_string();
                seen(&src, &mut participants);
                note(REPORTED, &src, format!("{} @{t}", told(&v, &ty(&v))), &mut lines);
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

    // Said in the drawing, across every lifeline: a diagram that starts late or stops early and
    // does not say so reads as a run in which nothing else happened.
    let across = match (names.first(), names.last()) {
        (Some(a), Some(b)) => format!("{a},{b}"),
        _ => String::new(),
    };
    for (i, l) in lines.iter().enumerate() {
        if opening == Some(i) && !across.is_empty() {
            doc.push_str(&format!(
                "    Note over {across}: the drawing starts here, at {opens_at}, {first} earlier messages not shown\n"
            ));
        }
        doc.push_str(l);
        doc.push('\n');
    }
    if landed > first + shown && !across.is_empty() {
        doc.push_str(&format!(
            "    Note over {across}: the drawing stops here, {shown} of {landed} messages shown\n"
        ));
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
    fn draw_around(name: &str, lines: &[&str], around: Option<u64>) -> String {
        let dir = std::env::temp_dir().join(format!("cuelight-viz-{name}"));
        let _ = std::fs::create_dir_all(&dir);
        let j = dir.join("journal.jsonl");
        let mut f = std::fs::File::create(&j).unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        drop(f);
        let out = dir.join("m.mmd");
        render(&j, &out, 100, around).unwrap();
        std::fs::read_to_string(out).unwrap()
    }

    fn draw(name: &str, lines: &[&str]) -> String {
        draw_around(name, lines, None)
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
            r#"{"t":10,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"rb"}}"#,
            r#"{"t":80,"kind":"recv","src":"n0","dest":"n1","mid":1,"body":{"type":"rb"}}"#,
        ]);
        assert!(d.contains("n0->>n1: rb 10 to 80"), "{d}");
    }

    /// Pairing on `mid` rather than oldest-first per link. The old heuristic was exactly right
    /// when the link was FIFO and wrong otherwise — and `fifo` is a scenario field meant to be
    /// turned off. Here the second message overtakes the first, and each label must still carry
    /// its own two instants.
    #[test]
    fn messages_pair_by_name_even_when_the_link_reorders_them() {
        let d = draw("reorder", &[
            r#"{"t":10,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"type":"a"}}"#,
            r#"{"t":20,"kind":"send","src":"n0","dest":"n1","mid":2,"body":{"type":"b"}}"#,
            r#"{"t":50,"kind":"recv","src":"n0","dest":"n1","mid":2,"body":{"type":"b"}}"#,
            r#"{"t":90,"kind":"recv","src":"n0","dest":"n1","mid":1,"body":{"type":"a"}}"#,
        ]);
        assert!(d.contains("n0->>n1: b 20 to 50"), "b left at 20 and landed at 50: {d}");
        assert!(d.contains("n0->>n1: a 10 to 90"), "a left at 10 and landed at 90: {d}");
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
    /// A report says what it reports: the type alone tells a reader that a process named a
    /// leader, never whom. Same for what the world asks, and for what a message carries.
    #[test]
    fn a_note_and_an_arrow_say_what_the_event_carries() {
        let d = draw(
            "carried",
            &[
                r#"{"t":1,"kind":"stimulus","dest":"n0","body":{"type":"set","value":42}}"#,
                r#"{"t":2,"kind":"observe","src":"n0","body":{"id":"n3","type":"leader"}}"#,
                r#"{"t":3,"kind":"send","src":"n0","dest":"n1","mid":1,"body":{"rank":7,"type":"hi"}}"#,
                r#"{"t":9,"kind":"recv","src":"n0","dest":"n1","mid":1,"body":{"rank":7,"type":"hi"}}"#,
            ],
        );
        assert!(d.contains("Note over n0: asked set value=42 @1"), "{d}");
        assert!(d.contains("Note over n0: leader id=n3 @2"), "{d}");
        assert!(d.contains("n0->>n1: hi rank=7"), "{d}");
    }

    /// A drawing that stops early says so, across every lifeline.
    #[test]
    fn a_drawing_that_is_cut_says_where() {
        // `draw` renders with a limit of 100 messages.
        let mut lines: Vec<String> = vec![];
        for i in 0..105 {
            lines.push(format!(
                r#"{{"t":{i},"kind":"recv","src":"n0","dest":"n1","mid":{i},"body":{{"type":"m"}}}}"#
            ));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let d = draw("cut", &refs);
        assert!(
            d.contains("Note over n0,n1: the drawing stops here, 100 of 105 messages shown"),
            "{}",
            &d[d.len().saturating_sub(300)..]
        );
        assert!(!draw("whole", &refs[..10]).contains("the drawing stops here"));
    }

    /// A run too long to draw whole is drawn around the instant the reader is after: mostly what
    /// led up to it, some of what followed, and a note at each end that is not an end of the run.
    #[test]
    fn a_long_run_is_drawn_around_the_instant_asked_for() {
        // `draw` renders with a limit of 100 messages: 75 up to the instant, 25 after it.
        let mut lines: Vec<String> = vec![
            r#"{"t":3,"kind":"fault-crash","detail":{"node":"n2"}}"#.into(),
            r#"{"t":4,"kind":"observe","src":"n0","body":{"type":"early"}}"#.into(),
        ];
        for i in 0..1000 {
            lines.push(format!(
                r#"{{"t":{i},"kind":"recv","src":"n0","dest":"n1","mid":{i},"body":{{"type":"m"}}}}"#
            ));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let d = draw_around("around", &refs, Some(500));
        assert!(d.contains("the drawing starts here, at 426, 426 earlier messages not shown"), "{d}");
        assert!(d.contains("n0->>n1: m @426") && d.contains("n0->>n1: m @525"), "{d}");
        assert!(!d.contains("m @425\n") && !d.contains("m @526\n"), "{d}");
        assert!(d.contains("the drawing stops here, 100 of 1000 messages shown"), "{d}");
        // What the network did to a process is kept from before the opening; a report is not.
        assert!(d.contains("CRASH @3"), "{d}");
        assert!(!d.contains("early"), "{d}");

        // Asked for the very end, the drawing is the last hundred and does not say it stops.
        let end = draw_around("around-end", &refs, Some(5000));
        assert!(end.contains("m @900") && end.contains("m @999"), "{end}");
        assert!(!end.contains("the drawing stops here"), "{end}");
        // A run that fits is drawn whole wherever the instant is.
        assert!(!draw_around("around-small", &refs[..50], Some(20)).contains("the drawing starts here"));
    }

    #[test]
    fn a_colon_in_a_label_does_not_truncate_it() {
        let d = draw("colon", &[r#"{"t":1,"kind":"observe","src":"n0","body":{"type":"a:b"}}"#]);
        assert!(d.contains("Note over n0: a b @1"), "{d}");
    }
}
