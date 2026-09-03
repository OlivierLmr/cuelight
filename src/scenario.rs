//! Scenarios: the replay unit.
//!
//! Deliberately *not* "a seed". Student code changes between runs, so a seed would be consumed
//! differently and the same seed would replay a different run. A scenario pins everything the
//! harness controls (GST, per-link delays, the fault schedule), so it replays against any version
//! of their code, can be shrunk, and can be pasted into a bug report.
//!
//! A seed *expands* into a scenario deterministically; the scenario is what gets stored.

use crate::rng::Rng;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Fault {
    /// Stops forever. No recovery, per the fault model.
    Crash { at: u64, node: String },
    /// Alive but processes nothing until `at + duration`. Its clock keeps running.
    Pause { at: u64, node: String, duration: u64 },
    /// Messages crossing the split are held, not dropped, until it heals.
    Partition { at: u64, duration: u64, side: Vec<String> },
}

impl Fault {
    pub fn at(&self) -> u64 {
        match self {
            Fault::Crash { at, .. } | Fault::Pause { at, .. } | Fault::Partition { at, .. } => *at,
        }
    }
}

/// A workload event: the harness poking a node to do something ("broadcast this", "you want the
/// critical section", "propose this value"). Carried by the scenario so the harness itself stays
/// lab-agnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stimulus {
    pub at: u64,
    pub node: String,
    pub body: Value,
}

/// A stimulus template, supplied by the lab. The harness ships none of its own.
///
/// The point of the split: `do_broadcast`, `request_cs` and `propose` are event names belonging to
/// *some* lab, and an event-name is exactly the kind of knowledge this tool must not carry. A lab
/// describes its workload in JSON and the harness expands it against the seed.
///
/// ```json
/// { "events": [ { "count": [3, 9], "at_frac": [0.0, 0.5],
///                 "body": { "type": "do_broadcast", "mid": "m<i>" } } ] }
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct StimulusSpec {
    pub events: Vec<StimulusRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StimulusRule {
    /// How many events to emit, drawn uniformly from `[lo, hi)`. Ignored when `per_node`.
    #[serde(default)]
    pub count: Option<[u64; 2]>,
    /// One event per node instead of a drawn count. The node is not chosen at random.
    #[serde(default)]
    pub per_node: bool,
    /// Absolute time window `[lo, hi)`. `lo == hi` pins the time and draws nothing.
    #[serde(default)]
    pub at: Option<[u64; 2]>,
    /// Time window as a fraction of `time_limit`. Mutually exclusive with `at`.
    #[serde(default)]
    pub at_frac: Option<[f64; 2]>,
    /// Message body. Strings containing `<i>` get the event index; an object `{"$rand": [lo, hi)}`
    /// becomes a drawn integer.
    pub body: Value,
}

impl StimulusSpec {
    pub fn from_json(s: &str) -> Result<StimulusSpec, String> {
        serde_json::from_str(s).map_err(|e| format!("bad stimulus spec: {e}"))
    }
}

/// Substitute `<i>` in strings and resolve `{"$rand": [lo, hi)}` objects.
///
/// Draw order is depth-first over the template, so a spec expands the same way every time.
fn render(t: &Value, i: u64, r: &mut Rng) -> Value {
    match t {
        Value::String(s) => Value::String(s.replace("<i>", &i.to_string())),
        Value::Array(a) => Value::Array(a.iter().map(|v| render(v, i, r)).collect()),
        Value::Object(o) => {
            if let Some(Value::Array(b)) = o.get("$rand") {
                if o.len() == 1 && b.len() == 2 {
                    let lo = b[0].as_u64().unwrap_or(0);
                    let hi = b[1].as_u64().unwrap_or(lo + 1);
                    return json!(r.range(lo, hi));
                }
            }
            Value::Object(o.iter().map(|(k, v)| (k.clone(), render(v, i, r))).collect())
        }
        other => other.clone(),
    }
}

fn d_nodes() -> usize { 4 }
fn d_f() -> usize { 1 }
fn d_gst() -> u64 { 2000 }
fn d_limit() -> u64 { 10_000 }
fn d_jitter() -> u64 { 100 }
fn d_pre() -> u64 { 200 }
fn d_post() -> u64 { 10 }

/// Every field has a default, so a hand-written directed test can be as small as
/// `{"nodes": 4, "stimuli": [...]}`, because authoring one should not mean typing two n x n matrices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    /// Seed this was expanded from, for provenance only; replay uses the fields below.
    #[serde(default)]
    pub seed: u64,
    #[serde(default = "d_nodes")]
    pub nodes: usize,
    #[serde(default = "d_f")]
    pub f: usize,
    /// Global Stabilisation Time: delays are bounded only after this.
    ///
    /// Note this is the *scheduled* GST. A `pause` landing after it also violates partial
    /// synchrony, so the **effective** GST is `max(gst, end of the last pause)`. See design.md.
    /// Checkers must date liveness deadlines from the last fault, never from this field.
    #[serde(default = "d_gst")]
    pub gst: u64,
    #[serde(default = "d_limit")]
    pub time_limit: u64,
    /// Per-link FIFO ordering. Lamport's mutex needs it; Ricart-Agrawala does not.
    #[serde(default)]
    pub fifo: bool,
    /// Used for any link the matrices below do not cover, so a hand-written scenario can omit them.
    #[serde(default = "d_pre")]
    pub delay_pre_default: u64,
    #[serde(default = "d_post")]
    pub delay_post_default: u64,
    /// `delay_pre[from][to]`, used for messages sent before GST. May be empty.
    #[serde(default)]
    pub delay_pre: Vec<Vec<u64>>,
    #[serde(default)]
    pub delay_post: Vec<Vec<u64>>,
    /// Extra delay per message, as a percentage of that link's base, hashed from
    /// `(link, index-on-link)` so reordering happens without consuming the global PRNG stream.
    ///
    /// Must scale with the base: a flat jitter smaller than the gap between two messages on a link
    /// can never reorder them, which would silently make `fifo` a no-op.
    #[serde(default = "d_jitter")]
    pub jitter_pct: u64,
    #[serde(default)]
    pub faults: Vec<Fault>,
    #[serde(default)]
    pub stimuli: Vec<Stimulus>,
}

pub struct ExpandOpts {
    pub nodes: usize,
    pub f: usize,
    pub time_limit: u64,
    pub fifo: bool,
    /// If false, expands a clean run with no faults.
    pub with_faults: bool,
    pub stimuli: Option<StimulusSpec>,
}

impl Scenario {
    pub fn expand(seed: u64, o: &ExpandOpts) -> Scenario {
        let mut r = Rng::new(seed);
        let n = o.nodes;
        let names: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();

        // GST somewhere in the first third of the run, so there is a meaningful window on each side.
        let gst = r.range(o.time_limit / 10, o.time_limit / 3);

        let mut delay_pre = vec![vec![0u64; n]; n];
        let mut delay_post = vec![vec![0u64; n]; n];
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                // Before GST delays are large and unbounded-feeling; after, small and bounded.
                delay_pre[i][j] = r.range(1, 400);
                delay_post[i][j] = r.range(1, 25);
            }
        }

        let mut faults = Vec::new();
        if o.with_faults {
            // At most f crashes: the model's whole premise.
            let crashes = r.range(0, (o.f + 1) as u64) as usize;
            let mut crashed: Vec<usize> = Vec::new();
            for _ in 0..crashes {
                let victim = r.range(0, n as u64) as usize;
                if crashed.contains(&victim) {
                    continue;
                }
                crashed.push(victim);
                // Crashes land in the first 70% of the run. A crash at 95% is untestable:
                // Omega only promises *eventual* detection, and by late in the run its timeouts
                // have doubled well past the time remaining.
                faults.push(Fault::Crash {
                    at: r.range(1, o.time_limit * 7 / 10),
                    node: names[victim].clone(),
                });
            }
            // Pauses and partitions are not crashes and are not budgeted against f.
            for _ in 0..r.range(0, 3) {
                let victim = r.range(0, n as u64) as usize;
                if crashed.contains(&victim) {
                    continue;
                }
                let at = r.range(1, o.time_limit * 6 / 10);
                faults.push(Fault::Pause { at, node: names[victim].clone(), duration: r.range(10, 400) });
            }
            if r.range(0, 2) == 1 {
                let cut = r.range(1, n as u64) as usize;
                // Heal well before the end. A partition still open at the time limit makes
                // convergence checks meaningless: Omega only promises to converge once the
                // network behaves again, so the run would end mid-disagreement.
                let at = r.range(1, o.time_limit * 6 / 10);
                let max_dur = (o.time_limit * 8 / 10).saturating_sub(at).max(51);
                faults.push(Fault::Partition {
                    at,
                    duration: r.range(50, max_dur.min(601)),
                    side: names[..cut].to_vec(),
                });
            }
            faults.sort_by_key(|f| f.at());
        }

        Scenario {
            seed,
            nodes: n,
            f: o.f,
            gst,
            time_limit: o.time_limit,
            fifo: o.fifo,
            delay_pre_default: d_pre(),
            delay_post_default: d_post(),
            delay_pre,
            delay_post,
            jitter_pct: 100,
            faults,
            stimuli: Self::workload(&mut r, o, &names),
        }
    }

    /// Expand the lab's stimulus template. No template, no stimuli: the harness invents none.
    ///
    /// Draw order is `count`, then per event `node` then `at` then the body's `$rand`s. It is part
    /// of the meaning of a seed: changing it would silently repoint every stored seed at a
    /// different run.
    fn workload(r: &mut Rng, o: &ExpandOpts, names: &[String]) -> Vec<Stimulus> {
        let Some(spec) = &o.stimuli else { return Vec::new() };
        let n = o.nodes as u64;
        let mut out: Vec<Stimulus> = Vec::new();
        for rule in &spec.events {
            let window = |r: &mut Rng| -> u64 {
                let (lo, hi) = match (&rule.at, &rule.at_frac) {
                    (Some([a, b]), _) => (*a, *b),
                    (None, Some([a, b])) => (
                        (*a * o.time_limit as f64) as u64,
                        (*b * o.time_limit as f64) as u64,
                    ),
                    (None, None) => (0, 0),
                };
                if hi > lo { r.range(lo, hi) } else { lo }
            };
            if rule.per_node {
                for (i, name) in names.iter().enumerate() {
                    let at = window(r);
                    out.push(Stimulus { at, node: name.clone(), body: render(&rule.body, i as u64, r) });
                }
            } else {
                let [lo, hi] = rule.count.unwrap_or([1, 2]);
                let k = if hi > lo { r.range(lo, hi) } else { lo };
                for i in 0..k {
                    let who = r.range(0, n) as usize;
                    let at = window(r);
                    out.push(Stimulus { at, node: names[who].clone(), body: render(&rule.body, i, r) });
                }
            }
        }
        out.sort_by_key(|s| s.at);
        out
    }

    /// Delay for the `idx`-th message on link `from -> to`, sent at `now`.
    ///
    /// Jitter is hashed from `(from, to, idx)` rather than drawn from the run's PRNG, so a student
    /// changing how many messages they send does not shift every other link's timing.
    pub fn delay(&self, from: usize, to: usize, idx: u64, now: u64) -> u64 {
        let (matrix, fallback) = if now < self.gst {
            (&self.delay_pre, self.delay_pre_default)
        } else {
            (&self.delay_post, self.delay_post_default)
        };
        let base = matrix
            .get(from)
            .and_then(|row| row.get(to))
            .copied()
            .filter(|d| *d > 0)
            .unwrap_or(fallback);
        let spread = base.saturating_mul(self.jitter_pct) / 100;
        if spread == 0 {
            return base.max(1);
        }
        let mut h = (from as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add((to as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
            .wrapping_add(idx.wrapping_mul(0x1656_67B1_9E37_79F9));
        h ^= h >> 29;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 32;
        (base + h % spread).max(1)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("scenario serialises")
    }

    pub fn from_json(s: &str) -> Result<Scenario, String> {
        serde_json::from_str(s).map_err(|e| format!("bad scenario: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opts(nodes: usize, with_faults: bool, stimuli: Option<StimulusSpec>) -> ExpandOpts {
        ExpandOpts { nodes, f: 1, time_limit: 10_000, fifo: true, with_faults, stimuli }
    }

    /// A golden test on the one thing that must never drift silently. Seeds are stored in
    /// scenarios, cited in handouts and used to reproduce student bugs; if expansion changes, all
    /// of those quietly start meaning something else. A failure here is either a real regression or
    /// a deliberate break that has to be announced.
    #[test]
    fn seed_expansion_is_pinned() {
        let sc = Scenario::expand(3, &opts(3, true, None));
        assert_eq!(sc.gst, 3208);
        assert_eq!(sc.delay_pre[0], vec![0, 130, 393]);
        assert_eq!(sc.faults.len(), 1);
        match &sc.faults[0] {
            Fault::Crash { at, node } => {
                assert_eq!((*at, node.as_str()), (229, "n1"));
            }
            other => panic!("expected a crash, got {other:?}"),
        }
    }

    #[test]
    fn the_stimulus_template_does_not_disturb_the_network() {
        // Workload draws come last in the stream, so a lab changing its template must not repoint
        // every other lab's seeds.
        let bare = Scenario::expand(7, &opts(4, true, None));
        let spec = StimulusSpec::from_json(
            r#"{"events":[{"count":[3,9],"at_frac":[0.0,0.5],"body":{"type":"x","id":"m<i>"}}]}"#,
        )
        .unwrap();
        let loaded = Scenario::expand(7, &opts(4, true, Some(spec)));
        assert_eq!(bare.gst, loaded.gst);
        assert_eq!(bare.delay_pre, loaded.delay_pre);
        assert_eq!(bare.delay_post, loaded.delay_post);
        assert_eq!(format!("{:?}", bare.faults), format!("{:?}", loaded.faults));
        assert!(bare.stimuli.is_empty() && !loaded.stimuli.is_empty());
    }

    #[test]
    fn no_template_means_no_stimuli() {
        // The tool ships no workload of its own: without a lab-supplied template it invents none.
        assert!(Scenario::expand(1, &opts(4, true, None)).stimuli.is_empty());
    }

    #[test]
    fn count_and_index_substitution() {
        let spec = StimulusSpec::from_json(
            r#"{"events":[{"count":[5,6],"at":[100,100],"body":{"type":"go","id":"m<i>"}}]}"#,
        )
        .unwrap();
        let sc = Scenario::expand(9, &opts(4, false, Some(spec)));
        assert_eq!(sc.stimuli.len(), 5, "count [5,6) must draw exactly 5");
        let ids: Vec<String> = sc
            .stimuli
            .iter()
            .map(|s| s.body["id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(ids, vec!["m0", "m1", "m2", "m3", "m4"]);
        assert!(sc.stimuli.iter().all(|s| s.at == 100), "at [100,100] pins the time");
    }

    #[test]
    fn per_node_emits_one_each_and_draws_values() {
        let spec = StimulusSpec::from_json(
            r#"{"events":[{"per_node":true,"at":[0,0],"body":{"type":"propose","value":{"$rand":[0,2]}}}]}"#,
        )
        .unwrap();
        let sc = Scenario::expand(1, &opts(4, false, Some(spec)));
        assert_eq!(sc.stimuli.len(), 4);
        let nodes: Vec<&str> = sc.stimuli.iter().map(|s| s.node.as_str()).collect();
        assert_eq!(nodes, vec!["n0", "n1", "n2", "n3"], "one per node, in order");
        for s in &sc.stimuli {
            let v = s.body["value"].as_u64().unwrap();
            assert!(v < 2, "$rand [0,2) gave {v}");
        }
    }

    #[test]
    fn at_frac_is_relative_to_the_time_limit() {
        let spec = StimulusSpec::from_json(
            r#"{"events":[{"count":[20,21],"at_frac":[0.0,0.5],"body":{"type":"x"}}]}"#,
        )
        .unwrap();
        let sc = Scenario::expand(4, &opts(4, false, Some(spec)));
        assert!(sc.stimuli.iter().all(|s| s.at < 5_000), "at_frac 0.5 of 10000 is 5000");
    }

    #[test]
    fn stimuli_come_out_sorted_by_time() {
        let spec = StimulusSpec::from_json(
            r#"{"events":[{"count":[30,31],"at_frac":[0.0,1.0],"body":{"type":"x"}}]}"#,
        )
        .unwrap();
        let sc = Scenario::expand(11, &opts(4, false, Some(spec)));
        assert!(sc.stimuli.windows(2).all(|w| w[0].at <= w[1].at));
    }

    #[test]
    fn no_faults_expands_a_clean_run() {
        assert!(Scenario::expand(5, &opts(4, false, None)).faults.is_empty());
    }

    #[test]
    fn faults_are_ordered_and_crashes_respect_f() {
        for seed in 1..60 {
            let sc = Scenario::expand(seed, &opts(4, true, None));
            assert!(sc.faults.windows(2).all(|w| w[0].at() <= w[1].at()));
            let crashes = sc.faults.iter().filter(|f| matches!(f, Fault::Crash { .. })).count();
            assert!(crashes <= sc.f, "seed {seed}: {crashes} crashes for f={}", sc.f);
        }
    }

    #[test]
    fn a_hand_written_scenario_needs_almost_nothing() {
        // The handout promises `{"nodes": 4, "stimuli": [...]}` is enough. Every other field
        // defaults, or authoring a directed test would mean typing two n x n matrices.
        let sc = Scenario::from_json(
            r#"{"nodes": 4, "stimuli": [{"at": 10, "node": "n0", "body": {"type": "x"}}]}"#,
        )
        .unwrap();
        assert_eq!(sc.nodes, 4);
        assert_eq!(sc.stimuli.len(), 1);
        assert!(sc.delay_pre.is_empty(), "matrices may be omitted");
        assert_eq!(sc.time_limit, 10_000);
    }

    #[test]
    fn a_bad_template_is_rejected_with_a_message() {
        assert!(StimulusSpec::from_json("{\"events\": 3}").is_err());
    }
}
