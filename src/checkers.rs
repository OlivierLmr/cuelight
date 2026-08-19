//! Property checkers.
//!
//! Read the journal after a run rather than observing it live, which keeps the simulator free of
//! per-lab knowledge.
//!
//! Safety checks apply unconditionally. **Liveness checks are dated from
//! `Scenario::effective_gst()`, never from `scenario.gst`** — a pause landing after GST legitimately
//! delays convergence, and a GST-dated deadline would fail correct implementations.

use crate::scenario::Scenario;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    Safety,
    Liveness,
}

#[derive(Debug)]
pub struct Check {
    pub name: &'static str,
    pub kind: Kind,
    pub ok: bool,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct Report {
    pub checks: Vec<Check>,
}

impl Report {
    fn add(&mut self, name: &'static str, kind: Kind, ok: bool, detail: String) {
        self.checks.push(Check { name, kind, ok, detail });
    }
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.ok)
    }
    pub fn safety_ok(&self) -> bool {
        self.checks.iter().filter(|c| c.kind == Kind::Safety).all(|c| c.ok)
    }
}

struct Events {
    observes: Vec<(u64, String, Value)>,
    stimuli: Vec<(u64, String, Value)>,
    crashed: HashMap<String, u64>,
    end: u64,
}

fn load(journal: &Path) -> Result<Events, String> {
    let f = std::fs::File::open(journal).map_err(|e| format!("{}: {e}", journal.display()))?;
    let mut ev =
        Events { observes: vec![], stimuli: vec![], crashed: HashMap::new(), end: 0 };
    for line in BufReader::new(f).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
        let t = v.get("t").and_then(Value::as_u64).unwrap_or(0);
        ev.end = ev.end.max(t);
        match v.get("kind").and_then(Value::as_str).unwrap_or("") {
            "observe" => {
                let src = v.get("src").and_then(Value::as_str).unwrap_or("").to_string();
                ev.observes.push((t, src, v.get("body").cloned().unwrap_or(Value::Null)));
            }
            "stimulus" => {
                let dst = v.get("dest").and_then(Value::as_str).unwrap_or("").to_string();
                ev.stimuli.push((t, dst, v.get("body").cloned().unwrap_or(Value::Null)));
            }
            "fault-crash" => {
                if let Some(n) = v.pointer("/detail/node").and_then(Value::as_str) {
                    ev.crashed.insert(n.to_string(), t);
                }
            }
            _ => {}
        }
    }
    Ok(ev)
}

fn ty(b: &Value) -> &str {
    b.get("type").and_then(Value::as_str).unwrap_or("")
}

fn correct_nodes(sc: &Scenario, ev: &Events) -> Vec<String> {
    (0..sc.nodes).map(|i| format!("n{i}")).filter(|n| !ev.crashed.contains_key(n)).collect()
}

pub fn run(lab: &str, journal: &Path, sc: &Scenario) -> Result<Report, String> {
    let ev = load(journal)?;
    let mut r = Report::default();
    match lab {
        "lab1" => lab1(&ev, sc, &mut r),
        "lab2" => lab2(&ev, sc, &mut r),
        "lab3" => lab3(&ev, sc, &mut r),
        "lab4" => lab4(&ev, sc, &mut r),
        o => return Err(format!("unknown lab {o} (lab1|lab2|lab3|lab4)")),
    }
    Ok(r)
}

fn lab1(ev: &Events, sc: &Scenario, r: &mut Report) {
    let correct = correct_nodes(sc, ev);

    // -- reliable broadcast: agreement ------------------------------------
    let mut got: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    for (_, src, b) in &ev.observes {
        if ty(b) == "deliver" {
            if let Some(mid) = b.get("mid").and_then(Value::as_str) {
                got.entry(src).or_default().insert(mid.to_string());
            }
        }
    }
    let sets: Vec<&BTreeSet<String>> =
        correct.iter().filter_map(|n| got.get(n.as_str())).collect();
    if sets.is_empty() {
        r.add("rb-agreement", Kind::Safety, true, "no deliveries in this run".into());
    } else {
        let all_same = sets.windows(2).all(|w| w[0] == w[1]);
        let complete = sets.len() == correct.len();
        r.add(
            "rb-agreement",
            Kind::Safety,
            all_same && complete,
            if all_same && complete {
                format!("{} correct nodes each delivered {}", sets.len(), sets[0].len())
            } else if !all_same {
                "correct nodes delivered different sets".into()
            } else {
                format!("only {}/{} correct nodes delivered anything", sets.len(), correct.len())
            },
        );
    }

    // -- reliable broadcast: validity -------------------------------------
    let asked: BTreeSet<String> = ev
        .stimuli
        .iter()
        .filter(|(_, _, b)| ty(b) == "do_broadcast")
        .filter_map(|(_, _, b)| b.get("mid").and_then(Value::as_str).map(str::to_string))
        .collect();
    if !asked.is_empty() {
        let spurious: Vec<&String> =
            got.values().flatten().filter(|m| !asked.contains(*m)).collect();
        r.add(
            "rb-validity",
            Kind::Safety,
            spurious.is_empty(),
            if spurious.is_empty() {
                format!("all deliveries trace to one of {} broadcasts", asked.len())
            } else {
                format!("delivered {} message(s) nobody broadcast", spurious.len())
            },
        );
    }

    // -- mutex progress ----------------------------------------------------
    // The liveness half. Without it the deadlock scenario reports all-green, since holding the
    // lock forever with a dead process violates no safety property at all.
    let asked_cs: BTreeSet<&str> = ev
        .stimuli
        .iter()
        .filter(|(_, _, b)| ty(b) == "request_cs")
        .map(|(_, n, _)| n.as_str())
        .collect();
    if !asked_cs.is_empty() {
        let entered: BTreeSet<&str> = ev
            .observes
            .iter()
            .filter(|(_, _, b)| ty(b) == "enter_cs")
            .map(|(_, n, _)| n.as_str())
            .collect();
        let starved: Vec<&&str> = asked_cs
            .iter()
            .filter(|n| correct.iter().any(|c| c == **n) && !entered.contains(**n))
            .collect();
        r.add(
            "mutex-progress",
            Kind::Liveness,
            starved.is_empty(),
            if starved.is_empty() {
                format!("all {} requesting correct nodes entered", asked_cs.len())
            } else {
                format!("{starved:?} asked for the lock and never got it")
            },
        );
    }

    // -- mutual exclusion --------------------------------------------------
    let mut spans: Vec<(u64, u64, String)> = vec![];
    let mut open: HashMap<String, u64> = HashMap::new();
    for (t, src, b) in &ev.observes {
        match ty(b) {
            "enter_cs" => {
                open.insert(src.clone(), *t);
            }
            "exit_cs" => {
                if let Some(s) = open.remove(src) {
                    spans.push((s, *t, src.clone()));
                }
            }
            _ => {}
        }
    }
    // Still inside at the end: held until it crashed, or until the run stopped.
    for (who, s) in open {
        let e = ev.crashed.get(&who).copied().unwrap_or(ev.end);
        spans.push((s, e, who));
    }
    spans.sort();
    let mut clash = None;
    for w in spans.windows(2) {
        if w[1].0 < w[0].1 {
            clash = Some((w[0].clone(), w[1].clone()));
            break;
        }
    }
    r.add(
        "mutual-exclusion",
        Kind::Safety,
        clash.is_none(),
        match &clash {
            None => format!("{} critical sections, none overlapping", spans.len()),
            Some((a, b)) => {
                format!("{} held [{},{}) while {} entered at {}", a.2, a.0, a.1, b.2, b.0)
            }
        },
    );
}

fn lab2(ev: &Events, sc: &Scenario, r: &mut Report) {
    let correct = correct_nodes(sc, ev);
    let after = sc.effective_gst();
    let mut last: BTreeMap<&str, (u64, String)> = BTreeMap::new();
    for (t, src, b) in &ev.observes {
        if ty(b) == "leader" {
            if let Some(id) = b.get("id").and_then(Value::as_str) {
                last.insert(src, (*t, id.to_string()));
            }
        }
    }
    let finals: BTreeMap<&String, &String> =
        correct.iter().filter_map(|n| last.get(n.as_str()).map(|(_, l)| (n, l))).collect();
    if finals.is_empty() {
        r.add("omega-agreement", Kind::Liveness, false, "nobody reported a leader".into());
        return;
    }
    let agreed = finals.values().collect::<BTreeSet<_>>().len() == 1;
    let ldr = (*finals.values().next().unwrap()).clone();
    r.add(
        "omega-agreement",
        Kind::Liveness,
        agreed && finals.len() == correct.len(),
        format!("final leaders {finals:?} (deadline dated from t={after})"),
    );
    r.add(
        "omega-elects-correct",
        Kind::Liveness,
        !ev.crashed.contains_key(&ldr),
        format!("leader {ldr} {}", if ev.crashed.contains_key(&ldr) { "IS CRASHED" } else { "is alive" }),
    );
}

fn lab3(ev: &Events, sc: &Scenario, r: &mut Report) {
    let correct = correct_nodes(sc, ev);
    let after = sc.effective_gst();
    let mut views: BTreeMap<&str, Vec<(u64, u64, u64)>> = BTreeMap::new(); // t, view, heard
    for (t, src, b) in &ev.observes {
        if ty(b) == "view" {
            let v = b.get("v").and_then(Value::as_u64).unwrap_or(0);
            let h = b.get("heard").and_then(Value::as_u64).unwrap_or(0);
            views.entry(src).or_default().push((*t, v, h));
        }
    }
    let mut monotone = true;
    for n in &correct {
        if let Some(vs) = views.get(n.as_str()) {
            if vs.windows(2).any(|w| w[1].1 < w[0].1) {
                monotone = false;
            }
        }
    }
    let maxv = views.values().flatten().map(|(_, v, _)| *v).max().unwrap_or(0);
    r.add("view-monotone", Kind::Safety, monotone, format!("max view {maxv}"));

    let need = correct.len() as u64;
    let mut per_view: BTreeMap<u64, u64> = BTreeMap::new();
    for n in &correct {
        for (t, v, h) in views.get(n.as_str()).map(|x| x.as_slice()).unwrap_or(&[]) {
            if *t >= after && *h >= need {
                *per_view.entry(*v).or_insert(0) += 1;
            }
        }
    }
    let good: Vec<u64> = per_view.iter().filter(|(_, c)| **c == need).map(|(v, _)| *v).collect();
    r.add(
        "view-co-presence",
        Kind::Liveness,
        !good.is_empty(),
        if good.is_empty() {
            format!("no view after t={after} heard from all {need} correct nodes")
        } else {
            format!("{} good views after t={after} (first: {})", good.len(), good[0])
        },
    );
}

fn lab4(ev: &Events, sc: &Scenario, r: &mut Report) {
    let correct = correct_nodes(sc, ev);
    let proposed: BTreeSet<i64> = ev
        .stimuli
        .iter()
        .filter(|(_, _, b)| ty(b) == "propose")
        .filter_map(|(_, _, b)| b.get("value").and_then(Value::as_i64))
        .collect();
    let mut decided: BTreeMap<&str, i64> = BTreeMap::new();
    for (_, src, b) in &ev.observes {
        if ty(b) == "decide" {
            if let Some(v) = b.get("value").and_then(Value::as_i64) {
                decided.entry(src).or_insert(v);
            }
        }
    }
    let distinct: BTreeSet<i64> = decided.values().copied().collect();
    r.add(
        "otr-agreement",
        Kind::Safety,
        distinct.len() <= 1,
        if distinct.len() <= 1 {
            format!("{} nodes decided, all on {distinct:?}", decided.len())
        } else {
            format!("CONFLICTING decisions {decided:?}")
        },
    );
    let bad: Vec<_> = decided.iter().filter(|(_, v)| !proposed.contains(v)).collect();
    r.add(
        "otr-validity",
        Kind::Safety,
        bad.is_empty(),
        if bad.is_empty() {
            format!("decisions lie in the proposals {proposed:?}")
        } else {
            format!("decided a value nobody proposed: {bad:?}")
        },
    );
    let all = decided.keys().filter(|k| correct.iter().any(|c| c == *k)).count() == correct.len();
    r.add(
        "otr-termination",
        Kind::Liveness,
        all,
        format!("{}/{} correct nodes decided", decided.len().min(correct.len()), correct.len()),
    );
}
