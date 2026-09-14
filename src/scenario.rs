//! Scenarios, and the spaces a seed draws them from.
//!
//! A **scenario** is the replay unit: it pins everything the harness controls, so it replays
//! against any version of the code under test, can be shrunk, and can be pasted into a bug report.
//! Deliberately *not* a seed: the code under test changes between runs, a seed would be consumed
//! differently, and the same seed would replay a different run.
//!
//! A seed draws a scenario from two spaces. An **environment space** describes what the system
//! undergoes: group size, delays, jitter, ordering, and the faults it must survive. A **workload
//! space** describes what it is asked to do. They are separate because they have different authors
//! and different lifetimes: the environment is a model of distributed computation, the workload
//! belongs to one algorithm.
//!
//! Every field of a space has the same shape, a [`Span`]: a scalar pins it, a two-element array
//! draws it from an inclusive range.

use crate::journal::canonical;
use crate::rng::Rng;
use serde::de;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};

/// Bumped whenever the draw changes shape.
///
/// Part of a scenario's fingerprint, so a stored run can say it came from a draw that no longer
/// exists rather than quietly meaning something else.
pub const DRAW_VERSION: u32 = 2;

// ---------------------------------------------------------------------- spans

/// A value that is either pinned or drawn from an inclusive range.
///
/// A scalar is a range of width zero, so a space has exactly one kind of value in it. A pinned
/// span consumes no randomness: pinning one parameter must never repoint the draws after it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Span<T> {
    Pinned(T),
    Range(T, T),
}

#[derive(Deserialize)]
#[serde(untagged)]
enum SpanRepr<T> {
    One(T),
    Two([T; 2]),
}

impl<'de, T> Deserialize<'de> for Span<T>
where
    T: Deserialize<'de> + PartialOrd + std::fmt::Debug,
{
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Ok(match SpanRepr::deserialize(d)? {
            SpanRepr::One(v) => Span::Pinned(v),
            SpanRepr::Two([a, b]) => {
                // Rejected rather than tolerated. A backwards range would otherwise pin silently
                // to its first element, and a typo in a bound is invisible from the results.
                if b < a {
                    return Err(de::Error::custom(format!(
                        "range [{a:?}, {b:?}] runs backwards; write it [low, high], inclusive"
                    )));
                }
                Span::Range(a, b)
            }
        })
    }
}

impl Span<u64> {
    /// Inclusive, unlike the exclusive `[lo, hi)` the generator speaks: a file edited by hand must
    /// not carry a second convention.
    pub fn draw(&self, r: &mut Rng) -> u64 {
        match *self {
            Span::Pinned(v) => v,
            Span::Range(lo, hi) if hi > lo => r.range(lo, hi + 1),
            Span::Range(lo, _) => lo,
        }
    }
}

impl Span<f64> {
    /// Quantised to a million steps, which keeps the draw integral and is finer than any fraction
    /// of a time limit needs.
    pub fn draw(&self, r: &mut Rng) -> f64 {
        const STEPS: u64 = 1_000_000;
        match *self {
            Span::Pinned(v) => v,
            Span::Range(lo, hi) if hi > lo => {
                lo + (hi - lo) * (r.range(0, STEPS + 1) as f64) / STEPS as f64
            }
            Span::Range(lo, _) => lo,
        }
    }
}

impl Span<bool> {
    pub fn draw(&self, r: &mut Rng) -> bool {
        match *self {
            Span::Pinned(v) => v,
            Span::Range(lo, hi) if lo != hi => r.range(0, 2) == 1,
            Span::Range(lo, _) => lo,
        }
    }
}

// --------------------------------------------------------------------- spaces

fn s_f() -> Span<u64> { Span::Pinned(1) }
fn s_limit() -> Span<u64> { Span::Pinned(10_000) }
fn s_gst_frac() -> Span<f64> { Span::Range(0.10, 0.33) }
fn s_pre() -> Span<u64> { Span::Range(1, 400) }
fn s_post() -> Span<u64> { Span::Range(1, 25) }
fn s_jitter() -> Span<u64> { Span::Pinned(100) }
fn s_fifo() -> Span<bool> { Span::Pinned(false) }
fn s_one() -> Span<u64> { Span::Range(0, 1) }
fn s_early() -> Span<f64> { Span::Range(0.0, 0.6) }
fn s_crash_at() -> Span<f64> { Span::Range(0.0, 0.7) }
fn s_pause_dur() -> Span<u64> { Span::Range(10, 400) }
fn s_part_dur() -> Span<u64> { Span::Range(50, 600) }

/// What the system undergoes. Shared across callers: it describes a model of computation, not an
/// algorithm, so the same file serves every suite that assumes that model.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSpace {
    /// The fault budget. The group size follows it, `n = 3f + 1`, so `[1, 3]` sweeps 4, 7 and 10.
    ///
    /// Declared this way round because `f` is the free parameter and `n` the consequence. A set of
    /// group sizes would also be the one place a space held a set rather than a range.
    #[serde(default = "s_f")]
    pub f: Span<u64>,
    #[serde(default = "s_limit")]
    pub time_limit: Span<u64>,
    /// GST as a fraction of the time limit, so there is a window on each side of it.
    #[serde(default = "s_gst_frac")]
    pub gst_frac: Span<f64>,
    /// Drawn once per ordered pair of nodes. Before GST delays are large, after it they are small.
    #[serde(default = "s_pre")]
    pub link_delay_pre: Span<u64>,
    #[serde(default = "s_post")]
    pub link_delay_post: Span<u64>,
    #[serde(default = "s_jitter")]
    pub jitter_pct: Span<u64>,
    #[serde(default = "s_fifo")]
    pub fifo: Span<bool>,
    /// Omitted means none happen. How many is not declared: it follows from `f`, which is the
    /// definition of the model rather than a setting.
    #[serde(default)]
    pub crashes: Option<Crashes>,
    /// Not budgeted against `f`: a pause is an omission, not a crash. Hence a count of its own.
    #[serde(default)]
    pub pauses: Option<Pauses>,
    #[serde(default)]
    pub partitions: Option<Partitions>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crashes {
    #[serde(default = "s_crash_at")]
    pub at_frac: Span<f64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Pauses {
    #[serde(default = "s_one")]
    pub count: Span<u64>,
    #[serde(default = "s_early")]
    pub at_frac: Span<f64>,
    #[serde(default = "s_pause_dur")]
    pub duration: Span<u64>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Partitions {
    #[serde(default = "s_one")]
    pub count: Span<u64>,
    #[serde(default = "s_early")]
    pub at_frac: Span<f64>,
    #[serde(default = "s_part_dur")]
    pub duration: Span<u64>,
}

impl EnvironmentSpace {
    pub fn from_json(s: &str) -> Result<EnvironmentSpace, String> {
        serde_json::from_str(s).map_err(|e| format!("bad environment space: {e}"))
    }
}

/// What the system is asked to do. Belongs to one caller: `do_broadcast`, `request_cs` and
/// `propose` are its event names, and an event name is exactly the knowledge this tool must not
/// carry.
///
/// ```json
/// { "stimuli": [ { "id": "req", "count": [3, 9], "at_frac": [0.0, 0.5],
///                  "body": { "type": "request_cs" } },
///                { "after": "req", "delay": [1, 20],
///                  "body": { "type": "request_cs" } } ] }
/// ```
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadSpace {
    pub stimuli: Vec<StimulusRule>,
}

/// One group of stimuli. Either free-standing, drawing its own count and instants, or following an
/// earlier group.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StimulusRule {
    /// Names this group so a later one may follow it.
    #[serde(default)]
    pub id: Option<String>,
    /// Follow an earlier group: one event per event of it, on the same node.
    ///
    /// Uniform instants hit "release, then immediately ask again" rarely, and that shape is where
    /// a whole class of bugs lives.
    #[serde(default)]
    pub after: Option<String>,
    /// How long after the event it follows. Only meaningful with `after`.
    #[serde(default)]
    pub delay: Option<Span<u64>>,
    #[serde(default)]
    pub count: Option<Span<u64>>,
    /// Instant as a fraction of the time limit. There is no absolute form: the limit lives in the
    /// environment, so an absolute instant would mean something different in each pairing.
    #[serde(default)]
    pub at_frac: Option<Span<f64>>,
    /// One event per node, in order, instead of a drawn count and a drawn node.
    #[serde(default)]
    pub per_node: bool,
    pub body: Value,
}

impl WorkloadSpace {
    pub fn from_json(s: &str) -> Result<WorkloadSpace, String> {
        let w: WorkloadSpace =
            serde_json::from_str(s).map_err(|e| format!("bad workload space: {e}"))?;
        w.validate()?;
        Ok(w)
    }

    /// Rejected here rather than ignored during the draw: a rule that cannot mean what it says is
    /// a mistake in the file, and a silent one would show up as a workload nobody asked for.
    fn validate(&self) -> Result<(), String> {
        let mut seen: Vec<&str> = Vec::new();
        for (i, rule) in self.stimuli.iter().enumerate() {
            match &rule.after {
                Some(target) => {
                    if !seen.contains(&target.as_str()) {
                        return Err(format!(
                            "stimulus group {i}: `after` names {target:?}, which is not an earlier \
                             group's `id`. Referring only backwards is what makes a cycle impossible."
                        ));
                    }
                    if rule.count.is_some() || rule.at_frac.is_some() || rule.per_node {
                        return Err(format!(
                            "stimulus group {i}: with `after`, the count and the instants come from \
                             the group it follows, so `count`, `at_frac` and `per_node` have no meaning"
                        ));
                    }
                }
                None => {
                    if rule.delay.is_some() {
                        return Err(format!(
                            "stimulus group {i}: `delay` is relative to the group named by `after`, \
                             and this group follows none"
                        ));
                    }
                }
            }
            if let Some(id) = &rule.id {
                if seen.contains(&id.as_str()) {
                    return Err(format!("stimulus group {i}: `id` {id:?} is already taken"));
                }
                seen.push(id);
            }
        }
        Ok(())
    }
}

/// A fingerprint of the spaces a scenario was drawn from, and of the draw itself.
///
/// Editing a space repoints every seed: seed 47 stops meaning the run it meant yesterday, and
/// nothing fails to say so. Carrying this makes that loud.
pub fn fingerprint(environment: &str, workload: Option<&str>) -> String {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01B3);
        }
    };
    eat(&DRAW_VERSION.to_le_bytes());
    for src in [Some(environment), workload].into_iter().flatten() {
        // Canonical bytes, so reformatting a space file is not mistaken for changing it.
        let text = match serde_json::from_str::<Value>(src) {
            Ok(v) => canonical(&v).to_string(),
            Err(_) => src.to_string(),
        };
        eat(text.as_bytes());
        eat(b"\0");
    }
    format!("{h:016x}")
}

// ------------------------------------------------------------------ scenarios

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

/// The harness poking a node to do something. Carried by the scenario, so the harness itself stays
/// workload-agnostic.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stimulus {
    pub at: u64,
    pub node: String,
    pub body: Value,
}

/// Where a drawn scenario came from. Absent on one written by hand, which came from nobody.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Drawn {
    pub seed: u64,
    pub environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload: Option<String>,
    /// See [`fingerprint`]. A seed names a run only relative to the spaces it was drawn from.
    pub fingerprint: String,
}

fn d_nodes() -> usize { 4 }
fn d_f() -> usize { 1 }
fn d_gst() -> u64 { 2000 }
fn d_limit() -> u64 { 10_000 }
fn d_jitter() -> u64 { 100 }
fn d_pre() -> u64 { 200 }
fn d_post() -> u64 { 10 }

/// Every field has a default, so a scenario written by hand can be as small as
/// `{"nodes": 4, "stimuli": [...]}`: authoring one should not mean typing two n x n matrices.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    /// Provenance only; replay uses the fields below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drawn: Option<Drawn>,
    #[serde(default = "d_nodes")]
    pub nodes: usize,
    #[serde(default = "d_f")]
    pub f: usize,
    /// Global Stabilisation Time: delays are bounded only after this.
    ///
    /// Note this is the *scheduled* GST. A `pause` landing after it also violates partial
    /// synchrony, so the **effective** GST is `max(gst, end of the last pause)`. Checkers must date
    /// liveness deadlines from the last fault, never from this field.
    #[serde(default = "d_gst")]
    pub gst: u64,
    #[serde(default = "d_limit")]
    pub time_limit: u64,
    /// Per-link FIFO ordering.
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
    /// `(link, index-on-link)` so reordering happens without consuming the run's PRNG stream.
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

impl Scenario {
    /// Draw a scenario from a seed and two spaces.
    ///
    /// The draw order is fixed here, by the order of these statements, and never by the order of
    /// keys in a space file: reordering two keys must not repoint every stored seed. It is part of
    /// what a seed means, so changing it means bumping [`DRAW_VERSION`].
    pub fn draw(seed: u64, env: &EnvironmentSpace, work: Option<&WorkloadSpace>) -> Scenario {
        let mut r = Rng::new(seed);

        // Capped so the subset mask a partition draws stays inside a u64. Far above any real group.
        let f = env.f.draw(&mut r).min(20);
        let n = (3 * f + 1) as usize;
        let names: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();

        let time_limit = env.time_limit.draw(&mut r).max(2);
        let gst = ((env.gst_frac.draw(&mut r) * time_limit as f64) as u64).min(time_limit);

        let mut delay_pre = vec![vec![0u64; n]; n];
        let mut delay_post = vec![vec![0u64; n]; n];
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                delay_pre[i][j] = env.link_delay_pre.draw(&mut r).max(1);
                delay_post[i][j] = env.link_delay_post.draw(&mut r).max(1);
            }
        }

        let jitter_pct = env.jitter_pct.draw(&mut r);
        let fifo = env.fifo.draw(&mut r);
        let last = time_limit - 1;

        let mut faults: Vec<Fault> = Vec::new();
        let mut crashed: Vec<usize> = Vec::new();

        if let Some(c) = &env.crashes {
            // Bounded by f: the model's premise, not a setting a space gets to make.
            let k = r.range(0, f + 1);
            for _ in 0..k {
                // Every value is drawn before anything is skipped, so a collision does not shift
                // the rest of the stream.
                let victim = r.range(0, n as u64) as usize;
                let at = ((c.at_frac.draw(&mut r) * time_limit as f64) as u64).clamp(1, last);
                if crashed.contains(&victim) {
                    continue;
                }
                crashed.push(victim);
                faults.push(Fault::Crash { at, node: names[victim].clone() });
            }
        }

        if let Some(p) = &env.pauses {
            let k = p.count.draw(&mut r);
            for _ in 0..k {
                let victim = r.range(0, n as u64) as usize;
                let at = ((p.at_frac.draw(&mut r) * time_limit as f64) as u64).clamp(1, last);
                let duration = p.duration.draw(&mut r).max(1);
                if crashed.contains(&victim) {
                    continue;
                }
                faults.push(Fault::Pause { at, node: names[victim].clone(), duration });
            }
        }

        if let Some(p) = &env.partitions {
            // Heal well before the end: a split still open at the limit makes any convergence
            // check meaningless, because the run would end mid-disagreement.
            let latest_heal = time_limit * 8 / 10;
            // Two sides need two nodes: a proper non-empty subset of a group of one has no answer.
            let k = if n < 2 { 0 } else { p.count.draw(&mut r) };
            for _ in 0..k {
                let at = ((p.at_frac.draw(&mut r) * time_limit as f64) as u64).clamp(1, last);
                let duration =
                    p.duration.draw(&mut r).max(1).min(latest_heal.saturating_sub(at).max(1));
                // A proper non-empty subset, drawn as a mask. Taking a prefix of the node list
                // instead would put n0 on the small side of every partition ever drawn.
                let mask = r.range(1, (1u64 << n) - 1);
                let side: Vec<String> =
                    (0..n).filter(|i| mask >> i & 1 == 1).map(|i| names[i].clone()).collect();
                faults.push(Fault::Partition { at, duration, side });
            }
        }

        faults.sort_by_key(|f| f.at());

        let stimuli = match work {
            None => Vec::new(),
            Some(w) => Self::workload(&mut r, w, &names, time_limit),
        };

        Scenario {
            drawn: None,
            nodes: n,
            f: f as usize,
            gst,
            time_limit,
            fifo,
            delay_pre_default: d_pre(),
            delay_post_default: d_post(),
            delay_pre,
            delay_post,
            jitter_pct,
            faults,
            stimuli,
        }
    }

    /// Record where this was drawn from. Kept off [`Scenario::draw`] so the draw stays a function
    /// of the spaces alone, not of where they happened to be stored.
    pub fn from(mut self, d: Drawn) -> Scenario {
        self.drawn = Some(d);
        self
    }

    /// Expand the caller's workload. No space, no stimuli: the harness invents none of its own.
    ///
    /// Groups are drawn in file order; within a group, `count` first, then per event the node, the
    /// instant, and the body's `$rand`s. A group that follows another draws one delay per event of
    /// the group it follows.
    fn workload(
        r: &mut Rng,
        w: &WorkloadSpace,
        names: &[String],
        time_limit: u64,
    ) -> Vec<Stimulus> {
        let n = names.len() as u64;
        let last = time_limit - 1;
        let mut out: Vec<Stimulus> = Vec::new();
        // Indices into `out`, resolved before the sort below, which is why `after` can address
        // them at all.
        let mut groups: Vec<(&str, Vec<usize>)> = Vec::new();

        for rule in &w.stimuli {
            let mut mine: Vec<usize> = Vec::new();
            match &rule.after {
                Some(target) => {
                    let base: Vec<usize> = groups
                        .iter()
                        .find(|(id, _)| *id == target.as_str())
                        .map(|(_, ix)| ix.clone())
                        .unwrap_or_default();
                    let span = rule.delay.unwrap_or(Span::Pinned(1));
                    for (i, bi) in base.into_iter().enumerate() {
                        let d = span.draw(r).max(1);
                        let at = out[bi].at.saturating_add(d).min(last);
                        let node = out[bi].node.clone();
                        let body = render(&rule.body, i as u64, r);
                        mine.push(out.len());
                        out.push(Stimulus { at, node, body });
                    }
                }
                None => {
                    if rule.per_node {
                        for (i, name) in names.iter().enumerate() {
                            let at = instant(r, &rule.at_frac, time_limit);
                            let body = render(&rule.body, i as u64, r);
                            mine.push(out.len());
                            out.push(Stimulus { at, node: name.clone(), body });
                        }
                    } else {
                        let k = rule.count.unwrap_or(Span::Pinned(1)).draw(r);
                        for i in 0..k {
                            let who = r.range(0, n) as usize;
                            let at = instant(r, &rule.at_frac, time_limit);
                            let body = render(&rule.body, i, r);
                            mine.push(out.len());
                            out.push(Stimulus { at, node: names[who].clone(), body });
                        }
                    }
                }
            }
            if let Some(id) = &rule.id {
                groups.push((id.as_str(), mine));
            }
        }

        out.sort_by_key(|s| s.at);
        out
    }

    /// Delay for the `idx`-th message on link `from -> to`, sent at `now`.
    ///
    /// Jitter is hashed from `(from, to, idx)` rather than drawn from the run's PRNG, so code under
    /// test changing how many messages it sends does not shift every other link's timing.
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

    /// The seed this was drawn from, or zero for one written by hand. Used to seed anything that
    /// must vary with the run without disturbing the draw itself.
    pub fn seed(&self) -> u64 {
        self.drawn.as_ref().map(|d| d.seed).unwrap_or(0)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).expect("scenario serialises")
    }

    pub fn from_json(s: &str) -> Result<Scenario, String> {
        serde_json::from_str(s).map_err(|e| format!("bad scenario: {e}"))
    }
}

fn instant(r: &mut Rng, at_frac: &Option<Span<f64>>, time_limit: u64) -> u64 {
    match at_frac {
        Some(s) => ((s.draw(r) * time_limit as f64) as u64).min(time_limit - 1),
        None => 0,
    }
}

/// Substitute `<i>` in strings and resolve `{"$rand": [lo, hi]}` objects, inclusive.
///
/// Keys are walked in sorted order, not document order. Depth-first over the document would make
/// swapping two keys in a body repoint every seed, silently.
fn render(t: &Value, i: u64, r: &mut Rng) -> Value {
    match t {
        Value::String(s) => Value::String(s.replace("<i>", &i.to_string())),
        Value::Array(a) => Value::Array(a.iter().map(|v| render(v, i, r)).collect()),
        Value::Object(o) => {
            if let Some(Value::Array(b)) = o.get("$rand") {
                if o.len() == 1 && b.len() == 2 {
                    let lo = b[0].as_u64().unwrap_or(0);
                    let hi = b[1].as_u64().unwrap_or(lo);
                    return json!(if hi > lo { r.range(lo, hi + 1) } else { lo });
                }
            }
            let mut keys: Vec<&String> = o.keys().collect();
            keys.sort();
            Value::Object(keys.into_iter().map(|k| (k.clone(), render(&o[k], i, r))).collect())
        }
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(src: &str) -> EnvironmentSpace {
        EnvironmentSpace::from_json(src).expect("environment parses")
    }
    fn work(src: &str) -> WorkloadSpace {
        WorkloadSpace::from_json(src).expect("workload parses")
    }

    /// A golden test on the one thing that must never drift silently. Seeds address stored runs and
    /// reproduce reported bugs; if the draw changes, all of those quietly start meaning something
    /// else. A failure here is either a regression or a deliberate break that has to be announced
    /// by bumping DRAW_VERSION.
    #[test]
    fn the_draw_is_pinned() {
        let e = env(r#"{"f": 1, "fifo": true, "crashes": {}}"#);
        let sc = Scenario::draw(3, &e, None);
        assert_eq!((sc.nodes, sc.f), (4, 1));
        assert_eq!(sc.gst, 2855);
        assert_eq!(sc.delay_pre[0], vec![0, 362, 48, 136]);
        assert_eq!(sc.faults.len(), 1);
        match &sc.faults[0] {
            Fault::Crash { at, node } => assert_eq!((*at, node.as_str()), (496, "n0")),
            other => panic!("expected a crash, got {other:?}"),
        }
    }

    #[test]
    fn a_scalar_and_a_range_of_width_zero_are_the_same_thing() {
        // Not just the same value: the same run. A width-zero range must draw nothing, or the two
        // ways of pinning a field would mean different things.
        let e = r#"{"f": F, "crashes": {}, "pauses": {"count": [0, 2]}}"#;
        for seed in 1..30 {
            let scalar = Scenario::draw(seed, &env(&e.replace("F", "2")), None);
            let range = Scenario::draw(seed, &env(&e.replace("F", "[2, 2]")), None);
            assert_eq!(scalar.to_json(), range.to_json(), "seed {seed}");
        }
    }

    #[test]
    fn changing_a_pinned_field_changes_that_field_and_nothing_else() {
        // True of fields carried straight into the scenario. `f` decides the group size and
        // `time_limit` scales every instant, so pinning either moves far more than itself.
        let base = r#"{"f": 2, "crashes": {}, "pauses": {"count": [0, 2]}, "#;
        let quiet = env(&format!(r#"{base}"jitter_pct": 100, "fifo": false}}"#));
        let loud = env(&format!(r#"{base}"jitter_pct": 900, "fifo": true}}"#));
        for seed in 1..30 {
            let a = Scenario::draw(seed, &quiet, None);
            let b = Scenario::draw(seed, &loud, None);
            assert_eq!((a.jitter_pct, a.fifo), (100, false));
            assert_eq!((b.jitter_pct, b.fifo), (900, true));
            assert_eq!((a.gst, &a.delay_pre), (b.gst, &b.delay_pre), "seed {seed}");
            assert_eq!(format!("{:?}", a.faults), format!("{:?}", b.faults), "seed {seed}");
        }
    }

    #[test]
    fn integer_ranges_are_inclusive() {
        // [2, 2] is one value, not an empty half-open interval, and [1, 3] can reach 3.
        let two = env(r#"{"f": [2, 2]}"#);
        assert_eq!(Scenario::draw(1, &two, None).f, 2);
        let sizes: Vec<usize> =
            (1..80).map(|s| Scenario::draw(s, &env(r#"{"f": [1, 3]}"#), None).nodes).collect();
        assert!(sizes.contains(&4) && sizes.contains(&7) && sizes.contains(&10));
        assert!(sizes.iter().all(|n| [4, 7, 10].contains(n)), "n = 3f+1 for f in 1..=3");
    }

    #[test]
    fn the_group_size_follows_the_fault_budget() {
        for f in 1..=4u64 {
            let sc = Scenario::draw(7, &env(&format!(r#"{{"f": {f}}}"#)), None);
            assert_eq!(sc.nodes, (3 * f + 1) as usize);
            assert_eq!(sc.f, f as usize);
        }
    }

    #[test]
    fn crashes_are_bounded_by_f_and_faults_are_ordered() {
        let e = env(r#"{"f": [1, 3], "crashes": {}, "pauses": {"count": [0, 2]}}"#);
        for seed in 1..120 {
            let sc = Scenario::draw(seed, &e, None);
            assert!(sc.faults.windows(2).all(|w| w[0].at() <= w[1].at()));
            let crashes = sc.faults.iter().filter(|f| matches!(f, Fault::Crash { .. })).count();
            assert!(crashes <= sc.f, "seed {seed}: {crashes} crashes for f={}", sc.f);
        }
    }

    #[test]
    fn a_partition_is_a_proper_non_empty_subset_and_not_always_a_prefix() {
        let e = env(r#"{"f": 3, "partitions": {"count": 1}}"#);
        let mut non_prefix = 0;
        for seed in 1..120 {
            let sc = Scenario::draw(seed, &e, None);
            let sides: Vec<&Vec<String>> = sc
                .faults
                .iter()
                .filter_map(|f| match f {
                    Fault::Partition { side, .. } => Some(side),
                    _ => None,
                })
                .collect();
            for side in sides {
                assert!(!side.is_empty() && side.len() < sc.nodes, "seed {seed}: {side:?}");
                let prefix: Vec<String> =
                    (0..side.len()).map(|i| format!("n{i}")).collect();
                if *side != prefix {
                    non_prefix += 1;
                }
            }
        }
        // Drawing a prefix would put n0 on the small side of every partition ever drawn, which is
        // what this replaced.
        assert!(non_prefix > 50, "only {non_prefix} partitions were not a prefix");
    }

    #[test]
    fn no_workload_means_no_stimuli() {
        // The tool ships none of its own: without a caller-supplied space it invents none.
        assert!(Scenario::draw(1, &env(r#"{"f": 1, "crashes": {}}"#), None).stimuli.is_empty());
    }

    #[test]
    fn the_workload_does_not_disturb_the_environment() {
        // Workload draws come last in the stream, so a caller editing its workload must not
        // repoint the faults and delays of every other one.
        let e = env(r#"{"f": 1, "crashes": {}, "pauses": {"count": [0, 2]}}"#);
        let bare = Scenario::draw(7, &e, None);
        let w = work(r#"{"stimuli":[{"count":[3,9],"at_frac":[0.0,0.5],"body":{"type":"x"}}]}"#);
        let loaded = Scenario::draw(7, &e, Some(&w));
        assert_eq!(bare.gst, loaded.gst);
        assert_eq!(bare.delay_pre, loaded.delay_pre);
        assert_eq!(format!("{:?}", bare.faults), format!("{:?}", loaded.faults));
        assert!(bare.stimuli.is_empty() && !loaded.stimuli.is_empty());
    }

    #[test]
    fn count_is_inclusive_and_the_index_substitutes() {
        let w = work(r#"{"stimuli":[{"count":[5,5],"at_frac":0.01,"body":{"id":"m<i>"}}]}"#);
        let sc = Scenario::draw(9, &env(r#"{"f": 1}"#), Some(&w));
        assert_eq!(sc.stimuli.len(), 5, "count [5,5] draws exactly five");
        let ids: Vec<&str> = sc.stimuli.iter().map(|s| s.body["id"].as_str().unwrap()).collect();
        assert_eq!(ids, vec!["m0", "m1", "m2", "m3", "m4"]);
        assert!(sc.stimuli.iter().all(|s| s.at == 100), "a pinned fraction pins the instant");
    }

    #[test]
    fn per_node_emits_one_each_in_order() {
        let w = work(
            r#"{"stimuli":[{"per_node":true,"at_frac":0.0,"body":{"v":{"$rand":[0,1]},"w":{"$rand":[7,7]}}}]}"#,
        );
        let sc = Scenario::draw(1, &env(r#"{"f": 1}"#), Some(&w));
        let nodes: Vec<&str> = sc.stimuli.iter().map(|s| s.node.as_str()).collect();
        assert_eq!(nodes, vec!["n0", "n1", "n2", "n3"]);
        // Inclusive: [0, 1] reaches both ends, and [7, 7] is the single value seven.
        assert!(sc.stimuli.iter().all(|s| s.body["v"].as_u64().unwrap() <= 1));
        assert!(sc.stimuli.iter().all(|s| s.body["w"].as_u64() == Some(7)));
    }

    #[test]
    fn a_following_group_lands_on_the_same_node_shortly_after() {
        let w = work(
            r#"{"stimuli":[
                 {"id":"a","count":[6,6],"at_frac":[0.0,0.4],"body":{"type":"x"}},
                 {"after":"a","delay":[5,9],"body":{"type":"y"}}]}"#,
        );
        let sc = Scenario::draw(4, &env(r#"{"f": 1}"#), Some(&w));
        assert_eq!(sc.stimuli.len(), 12, "one follower per event of the group it follows");
        for f in sc.stimuli.iter().filter(|s| s.body["type"] == "y") {
            let ok = sc.stimuli.iter().any(|b| {
                b.body["type"] == "x" && b.node == f.node && (5..=9).contains(&(f.at - b.at))
            });
            assert!(ok, "no x on {} within 5..=9 before {}", f.node, f.at);
        }
    }

    #[test]
    fn reordering_two_keys_in_a_body_changes_nothing() {
        // Measured to change every seed before keys were walked in sorted order.
        let one = work(r#"{"stimuli":[{"count":3,"at_frac":0.1,"body":{"a":{"$rand":[0,999]},"b":{"$rand":[0,999]}}}]}"#);
        let other = work(r#"{"stimuli":[{"count":3,"at_frac":0.1,"body":{"b":{"$rand":[0,999]},"a":{"$rand":[0,999]}}}]}"#);
        let e = env(r#"{"f": 1}"#);
        let l = Scenario::draw(1, &e, Some(&one));
        let r = Scenario::draw(1, &e, Some(&other));
        let vals = |s: &Scenario| -> Vec<(u64, u64)> {
            s.stimuli
                .iter()
                .map(|x| (x.body["a"].as_u64().unwrap(), x.body["b"].as_u64().unwrap()))
                .collect()
        };
        assert_eq!(vals(&l), vals(&r));
    }

    #[test]
    fn stimuli_come_out_sorted_by_time() {
        let w = work(r#"{"stimuli":[{"count":[30,30],"at_frac":[0.0,1.0],"body":{"type":"x"}}]}"#);
        let sc = Scenario::draw(11, &env(r#"{"f": 1}"#), Some(&w));
        assert!(sc.stimuli.windows(2).all(|w| w[0].at <= w[1].at));
    }

    #[test]
    fn a_workload_that_cannot_mean_what_it_says_is_rejected() {
        let cases = [
            (r#"{"stimuli":[{"after":"nope","body":{}}]}"#, "not an earlier group"),
            (
                r#"{"stimuli":[{"id":"a","count":2,"body":{}},{"after":"a","count":3,"body":{}}]}"#,
                "have no meaning",
            ),
            (r#"{"stimuli":[{"delay":[1,2],"body":{}}]}"#, "follows none"),
            (
                r#"{"stimuli":[{"id":"a","body":{}},{"id":"a","body":{}}]}"#,
                "already taken",
            ),
            // Only backwards: a forward reference is how a cycle would start.
            (
                r#"{"stimuli":[{"after":"b","body":{}},{"id":"b","body":{}}]}"#,
                "not an earlier group",
            ),
        ];
        for (src, needle) in cases {
            let err = WorkloadSpace::from_json(src).expect_err(&format!("{src} must be rejected"));
            assert!(err.contains(needle), "{err:?} does not mention {needle:?}");
        }
    }

    #[test]
    fn an_unknown_field_is_a_mistake_not_a_silence() {
        assert!(EnvironmentSpace::from_json(r#"{"crash": {}}"#).is_err());
        assert!(WorkloadSpace::from_json(r#"{"events":[]}"#).is_err());
    }

    #[test]
    fn the_fingerprint_follows_meaning_not_formatting() {
        let a = r#"{"f": 1, "fifo": true}"#;
        let reformatted = "{\n  \"fifo\"  : true,\n  \"f\": 1\n}";
        let changed = r#"{"f": 2, "fifo": true}"#;
        assert_eq!(fingerprint(a, None), fingerprint(reformatted, None));
        assert_ne!(fingerprint(a, None), fingerprint(changed, None));
        // A workload is part of what a seed means, so adding one must move the fingerprint.
        assert_ne!(fingerprint(a, None), fingerprint(a, Some(r#"{"stimuli":[]}"#)));
    }


    // ------------------------------------------------------------- a sweep

    /// Environment spaces chosen to reach every branch of the draw, including the degenerate ones.
    const ENVIRONMENTS: &[&str] = &[
        r#"{}"#,
        r#"{"f": 0, "partitions": {"count": 2}}"#,
        r#"{"f": [0, 3], "crashes": {}, "pauses": {"count": [0, 3]}, "partitions": {"count": [0, 2]}}"#,
        r#"{"f": 3, "crashes": {"at_frac": [0.0, 1.0]}}"#,
        r#"{"f": [1, 2], "time_limit": [200, 4000], "gst_frac": [0.0, 1.0]}"#,
        r#"{"f": 1, "fifo": [false, true], "jitter_pct": [0, 300]}"#,
        r#"{"f": 2, "link_delay_pre": 7, "link_delay_post": [0, 0]}"#,
        r#"{"f": 1, "time_limit": 2, "crashes": {}, "pauses": {"count": 2}, "partitions": {"count": 2}}"#,
        r#"{"f": [1, 3], "partitions": {"count": 3, "at_frac": [0.0, 1.0], "duration": [1, 9000]}}"#,
    ];

    const WORKLOADS: &[&str] = &[
        r#"{"stimuli": []}"#,
        r#"{"stimuli":[{"count":[0,6],"at_frac":[0.0,1.0],"body":{"type":"x","v":{"$rand":[0,9]}}}]}"#,
        r#"{"stimuli":[{"per_node":true,"at_frac":[0.0,0.9],"body":{"type":"p","id":"m<i>"}}]}"#,
        r#"{"stimuli":[{"id":"a","count":[1,4],"at_frac":[0.9,1.0],"body":{"type":"a"}},
                       {"after":"a","delay":[1,5000],"body":{"type":"b"}}]}"#,
        r#"{"stimuli":[{"id":"a","count":[1,3],"at_frac":[0.0,0.2],"body":{"type":"a"}},
                       {"id":"b","after":"a","delay":[1,10],"body":{"type":"b"}},
                       {"after":"b","delay":[1,10],"body":{"type":"c"}}]}"#,
        r#"{"stimuli":[{"id":"a","count":0,"at_frac":0.1,"body":{"type":"a"}},
                       {"after":"a","delay":[1,10],"body":{"type":"b"}}]}"#,
    ];

    /// Every invariant the rest of the tool assumes, over every branch, on many seeds.
    ///
    /// The targeted tests above each pin one behaviour. This is what says no combination of space
    /// and seed produces something the simulator or a checker would have to cope with: an instant
    /// outside the run, a partition with nobody on one side, a node name that does not exist.
    #[test]
    fn the_draw_holds_its_invariants_everywhere() {
        for (ei, esrc) in ENVIRONMENTS.iter().enumerate() {
            let e = env(esrc);
            for (wi, wsrc) in WORKLOADS.iter().enumerate() {
                let w = work(wsrc);
                for seed in 1..40u64 {
                    let where_ = format!("env {ei}, workload {wi}, seed {seed}");
                    let sc = Scenario::draw(seed, &e, Some(&w));
                    let last = sc.time_limit - 1;
                    let names: Vec<String> = (0..sc.nodes).map(|i| format!("n{i}")).collect();

                    assert_eq!(sc.nodes, 3 * sc.f + 1, "{where_}: n = 3f+1");
                    assert!(sc.time_limit >= 2 && sc.gst <= sc.time_limit, "{where_}");

                    assert_eq!(sc.delay_pre.len(), sc.nodes, "{where_}");
                    for i in 0..sc.nodes {
                        for j in 0..sc.nodes {
                            let (a, b) = (sc.delay_pre[i][j], sc.delay_post[i][j]);
                            if i == j {
                                assert_eq!((a, b), (0, 0), "{where_}: a node delays to itself");
                            } else {
                                assert!(a >= 1 && b >= 1, "{where_}: zero delay on {i}->{j}");
                            }
                        }
                    }

                    let mut crashed: Vec<&str> = vec![];
                    for f in &sc.faults {
                        assert!((1..=last).contains(&f.at()), "{where_}: fault at {}", f.at());
                        match f {
                            Fault::Crash { node, .. } => {
                                assert!(names.contains(node), "{where_}: crash on {node}");
                                assert!(!crashed.contains(&node.as_str()), "{where_}: twice");
                                crashed.push(node);
                            }
                            Fault::Pause { node, duration, .. } => {
                                assert!(names.contains(node), "{where_}: pause on {node}");
                                assert!(*duration >= 1, "{where_}: pause of nothing");
                            }
                            Fault::Partition { at, duration, side } => {
                                assert!(!side.is_empty(), "{where_}: partition with an empty side");
                                assert!(side.len() < sc.nodes, "{where_}: partition of everyone");
                                assert!(side.iter().all(|s| names.contains(s)), "{where_}");
                                let mut seen = side.clone();
                                seen.sort();
                                seen.dedup();
                                assert_eq!(seen.len(), side.len(), "{where_}: a node listed twice");
                                assert!(*duration >= 1, "{where_}");
                                // Heals before the end, unless it started at or after the
                                // deadline, where a split cannot last less than one tick.
                                let heal = sc.time_limit * 8 / 10;
                                assert!(
                                    *at >= heal || at + duration <= heal,
                                    "{where_}: split {at}+{duration} outlives {heal}"
                                );
                            }
                        }
                    }
                    let n_crashes =
                        sc.faults.iter().filter(|f| matches!(f, Fault::Crash { .. })).count();
                    assert!(n_crashes <= sc.f, "{where_}: {n_crashes} crashes for f={}", sc.f);
                    assert!(sc.faults.windows(2).all(|p| p[0].at() <= p[1].at()), "{where_}");
                    for f in &sc.faults {
                        if let Fault::Pause { node, .. } = f {
                            assert!(!crashed.contains(&node.as_str()), "{where_}: pausing the dead");
                        }
                    }

                    for st in &sc.stimuli {
                        assert!(st.at <= last, "{where_}: stimulus at {} past {last}", st.at);
                        assert!(names.contains(&st.node), "{where_}: stimulus for {}", st.node);
                    }
                    assert!(sc.stimuli.windows(2).all(|p| p[0].at <= p[1].at), "{where_}: unsorted");

                    // The whole point of the tool: the same inputs give the same run.
                    let again = Scenario::draw(seed, &e, Some(&w));
                    assert_eq!(sc.to_json(), again.to_json(), "{where_}: draw is not a function");
                }
            }
        }
    }

    // -------------------------------------------------- paths the sweep cannot assert

    #[test]
    fn a_drawn_fraction_covers_its_range() {
        // Pinned would satisfy every invariant above while making gst_frac a dead field.
        let e = env(r#"{"f": 1, "time_limit": 10000, "gst_frac": [0.10, 0.33]}"#);
        let gsts: Vec<u64> = (1..200).map(|s| Scenario::draw(s, &e, None).gst).collect();
        assert!(gsts.iter().all(|g| (1000..=3300).contains(g)), "outside [0.10, 0.33]");
        let (lo, hi) = (gsts.iter().min().unwrap(), gsts.iter().max().unwrap());
        assert!(*lo < 1200 && *hi > 3100, "only reached {lo}..{hi} of 1000..3300");
    }

    #[test]
    fn a_drawn_flag_reaches_both_values() {
        let e = env(r#"{"f": 1, "fifo": [false, true]}"#);
        let seen: Vec<bool> = (1..40).map(|s| Scenario::draw(s, &e, None).fifo).collect();
        assert!(seen.contains(&true) && seen.contains(&false));
        // And pinning it still means pinning it.
        let pinned = env(r#"{"f": 1, "fifo": true}"#);
        assert!((1..40).all(|s| Scenario::draw(s, &pinned, None).fifo));
    }

    #[test]
    fn the_time_limit_can_be_drawn_and_everything_follows_it() {
        let e = env(r#"{"f": 1, "time_limit": [500, 600], "gst_frac": 0.5, "crashes": {}}"#);
        for seed in 1..40 {
            let sc = Scenario::draw(seed, &e, None);
            assert!((500..=600).contains(&sc.time_limit));
            assert_eq!(sc.gst, sc.time_limit / 2);
            assert!(sc.faults.iter().all(|f| f.at() < sc.time_limit));
        }
    }

    #[test]
    fn a_backwards_range_is_refused_for_every_kind_of_value() {
        let cases = [
            r#"{"link_delay_pre": [400, 1]}"#,
            r#"{"gst_frac": [0.9, 0.1]}"#,
            r#"{"fifo": [true, false]}"#,
        ];
        for src in cases {
            let err = EnvironmentSpace::from_json(src).expect_err(&format!("{src} must be refused"));
            assert!(err.contains("runs backwards"), "{err}");
        }
        // The right way round is fine, including a range of width zero.
        assert!(EnvironmentSpace::from_json(r#"{"link_delay_pre": [7, 7]}"#).is_ok());
    }

    #[test]
    fn a_group_of_one_node_cannot_be_partitioned() {
        // f = 0 leaves a single node. Asking for a proper non-empty subset of it has no answer,
        // and used to panic inside the generator rather than say so.
        let sc = Scenario::draw(1, &env(r#"{"f": 0, "partitions": {"count": 3}}"#), None);
        assert_eq!(sc.nodes, 1);
        assert!(sc.faults.is_empty());
    }

    #[test]
    fn a_follower_that_would_land_past_the_end_is_brought_back_inside() {
        // Otherwise the count of stimuli would depend on the delay drawn, and a workload would
        // quietly shrink near the time limit.
        let w = work(
            r#"{"stimuli":[{"id":"a","count":[4,4],"at_frac":[0.99,1.0],"body":{"type":"a"}},
                           {"after":"a","delay":[5000,9000],"body":{"type":"b"}}]}"#,
        );
        let sc = Scenario::draw(2, &env(r#"{"f": 1, "time_limit": 1000}"#), Some(&w));
        assert_eq!(sc.stimuli.len(), 8, "a follower must never be dropped");
        assert!(sc.stimuli.iter().all(|s| s.at <= 999));
    }

    #[test]
    fn a_chain_of_followers_resolves_in_order() {
        let w = work(
            r#"{"stimuli":[{"id":"a","count":[3,3],"at_frac":[0.0,0.1],"body":{"type":"a"}},
                           {"id":"b","after":"a","delay":[10,10],"body":{"type":"b"}},
                           {"after":"b","delay":[20,20],"body":{"type":"c"}}]}"#,
        );
        let sc = Scenario::draw(5, &env(r#"{"f": 1}"#), Some(&w));
        assert_eq!(sc.stimuli.len(), 9);
        for c in sc.stimuli.iter().filter(|s| s.body["type"] == "c") {
            let b = sc
                .stimuli
                .iter()
                .find(|s| s.body["type"] == "b" && s.node == c.node && s.at + 20 == c.at)
                .expect("no b twenty before its c, on the same node");
            assert!(sc
                .stimuli
                .iter()
                .any(|a| a.body["type"] == "a" && a.node == b.node && a.at + 10 == b.at));
        }
    }

    #[test]
    fn following_a_group_that_produced_nothing_produces_nothing() {
        let w = work(
            r#"{"stimuli":[{"id":"a","count":0,"at_frac":0.1,"body":{"type":"a"}},
                           {"after":"a","delay":[1,10],"body":{"type":"b"}}]}"#,
        );
        assert!(Scenario::draw(1, &env(r#"{"f": 1}"#), Some(&w)).stimuli.is_empty());
    }

    #[test]
    fn provenance_survives_a_round_trip() {
        let e = env(r#"{"f": 1}"#);
        let sc = Scenario::draw(3, &e, None).from(Drawn {
            seed: 3,
            environment: "environments/clean.json".into(),
            workload: None,
            fingerprint: fingerprint(r#"{"f": 1}"#, None),
        });
        let back = Scenario::from_json(&sc.to_json()).expect("round trip");
        assert_eq!(back.seed(), 3);
        let d = back.drawn.expect("provenance kept");
        assert_eq!((d.seed, d.environment.as_str()), (3, "environments/clean.json"));
        assert_eq!(d.fingerprint, fingerprint(r#"{"f": 1}"#, None));
    }

    #[test]
    fn a_hand_written_scenario_needs_almost_nothing_and_has_no_provenance() {
        let sc = Scenario::from_json(
            r#"{"nodes": 4, "stimuli": [{"at": 10, "node": "n0", "body": {"type": "x"}}]}"#,
        )
        .unwrap();
        assert_eq!((sc.nodes, sc.time_limit, sc.stimuli.len()), (4, 10_000, 1));
        assert!(sc.delay_pre.is_empty(), "matrices may be omitted");
        assert!(sc.drawn.is_none(), "written by hand, so drawn from nobody");
        assert_eq!(sc.seed(), 0);
    }
}
