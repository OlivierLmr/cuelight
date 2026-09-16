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
pub const DRAW_VERSION: u32 = 3;

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

impl<'de, T> Deserialize<'de> for Span<T>
where
    T: serde::de::DeserializeOwned + PartialOrd + std::fmt::Debug,
{
    /// Hand-written rather than `#[serde(untagged)]`, which swallows the inner error and reports
    /// only "data did not match any variant". The author needs to be told what was wrong.
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = Value::deserialize(d)?;
        let pair = match &v {
            Value::Array(a) if a.len() == 2 => Some((a[0].clone(), a[1].clone())),
            Value::Array(a) => {
                return Err(de::Error::custom(format!(
                    "a range is [low, high]; got {} elements",
                    a.len()
                )))
            }
            _ => None,
        };
        Ok(match pair {
            None => Span::Pinned(serde_json::from_value(v).map_err(de::Error::custom)?),
            Some((x, y)) => {
                let a: T = serde_json::from_value(x).map_err(de::Error::custom)?;
                let b: T = serde_json::from_value(y).map_err(de::Error::custom)?;
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


// --------------------------------------------------------------------- bounds

/// An integer bound, which may be the fault budget rather than a literal.
///
/// `f` is the only symbol the format admits and there is no arithmetic: `[0, "f"]` is the crash
/// budget, `[1, "f"]` demands at least one. Writing the bound is what buys the ability to *demand*
/// a crash rather than hope one is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Bound {
    Fixed(u64),
    F,
}

impl Bound {
    fn get(self, f: u64) -> u64 {
        match self {
            Bound::Fixed(v) => v,
            Bound::F => f,
        }
    }
}

/// `f` sorts above every literal, so the backwards-range check accepts `[0, "f"]` and still
/// rejects `["f", 0]`. Exact only because no space ever writes a literal above the budget.
impl PartialOrd for Bound {
    fn partial_cmp(&self, other: &Bound) -> Option<std::cmp::Ordering> {
        use std::cmp::Ordering::*;
        Some(match (self, other) {
            (Bound::Fixed(a), Bound::Fixed(b)) => a.cmp(b),
            (Bound::F, Bound::F) => Equal,
            (Bound::Fixed(_), Bound::F) => Less,
            (Bound::F, Bound::Fixed(_)) => Greater,
        })
    }
}

impl<'de> Deserialize<'de> for Bound {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::Number(n) if n.is_u64() => Ok(Bound::Fixed(n.as_u64().unwrap())),
            Value::String(s) if s == "f" => Ok(Bound::F),
            other => Err(de::Error::custom(format!(
                "expected an integer or \"f\", got {other}"
            ))),
        }
    }
}

impl Span<Bound> {
    fn draw(&self, r: &mut Rng, f: u64) -> u64 {
        match *self {
            Span::Pinned(v) => v.get(f),
            Span::Range(lo, hi) => {
                let (lo, hi) = (lo.get(f), hi.get(f));
                if hi > lo { r.range(lo, hi + 1) } else { lo }
            }
        }
    }
}

// ---------------------------------------------------------------------- space

fn s_f() -> Span<u64> { Span::Pinned(1) }
fn s_limit() -> Span<u64> { Span::Pinned(10_000) }
fn s_gst_frac() -> Span<f64> { Span::Range(0.10, 0.33) }
fn s_pre() -> Span<u64> { Span::Range(1, 400) }
fn s_post() -> Span<u64> { Span::Range(1, 25) }
fn s_jitter() -> Span<u64> { Span::Pinned(100) }
fn s_emit() -> Span<u64> { Span::Range(1, 12) }
fn s_fifo() -> Span<bool> { Span::Pinned(false) }

/// The set of scenarios a seed draws from: the world a run happens in, and a tree of events.
///
/// One file, not two. Faults and stimuli are events of the same tree, because a crash and a
/// request both have an instant and a subject, and separating them is what made "crash the sender
/// while it is broadcasting" impossible to say.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Space {
    /// The fault budget. The group size follows it, `n = 3f + 1`.
    #[serde(default = "s_f")]
    pub f: Span<u64>,
    #[serde(default = "s_limit")]
    pub time_limit: Span<u64>,
    #[serde(default = "s_gst_frac")]
    pub gst_frac: Span<f64>,
    /// Drawn once per ordered pair of processes.
    #[serde(default = "s_pre")]
    pub link_delay_pre: Span<u64>,
    #[serde(default = "s_post")]
    pub link_delay_post: Span<u64>,
    #[serde(default = "s_jitter")]
    pub jitter_pct: Span<u64>,
    /// How long a process takes between two of its own sends.
    ///
    /// A node's step is atomic: it emits every send at once. Without this they all leave at the
    /// same instant, nothing can happen between them, and the very case reliable broadcast exists
    /// to repair — a sender that dies partway through its send loop — cannot occur at all.
    #[serde(default = "s_emit")]
    pub emit_gap: Span<u64>,
    #[serde(default = "s_fifo")]
    pub fifo: Span<bool>,
    #[serde(default)]
    pub events: Vec<Event>,
}

/// Who an event acts on, and how many copies of it there are.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Subject {
    /// Every process, once each.
    #[serde(rename = "all")]
    All,
    /// k processes not already taken on the path from the root, so under a parent it reads as
    /// "someone other than mine".
    Distinct(Span<Bound>),
    /// k draws with replacement; the parent's own process may come up again.
    Any(Span<Bound>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    /// Body handed to the node untouched. The tool never reads it: an event name belongs to a
    /// caller, and that is knowledge this crate must not hold.
    Stimulus(Value),
    Crash,
    Pause { duration: Span<u64> },
    /// Cuts a proper non-empty subset from the rest. Its size is drawn, not declared, because
    /// that is what a partition *is*.
    Partition { duration: Span<u64> },
}

/// One node of the event tree.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// Binds a process, which children inherit unless they name their own. Absent on a child means
    /// "my parent's process"; absent on a root means the effect acts on no one in particular.
    #[serde(default)]
    pub nodes: Option<Subject>,
    /// How many copies, for an effect that acts on no single process. Mutually exclusive with
    /// `nodes`.
    #[serde(default)]
    pub count: Option<Span<Bound>>,
    /// Roots only: a fraction of the time limit.
    #[serde(default)]
    pub at_frac: Option<Span<f64>>,
    /// Children only: how long after the parent. Never negative, so a child never precedes its
    /// parent and the tree admits no cycle.
    #[serde(default)]
    pub delay: Option<Span<u64>>,
    #[serde(flatten)]
    pub effect: Effect,
    #[serde(default)]
    pub then: Vec<Event>,
}

impl Space {
    pub fn from_json(s: &str) -> Result<Space, String> {
        let sp: Space = serde_json::from_str(s).map_err(|e| format!("bad space: {e}"))?;
        sp.validate()?;
        Ok(sp)
    }

    /// Rejected at read time rather than ignored during the draw: a field that cannot mean what it
    /// says is a mistake in the file, and a silent one shows up as a run nobody asked for.
    fn validate(&self) -> Result<(), String> {
        fn walk(events: &[Event], root: bool, where_: &str) -> Result<(), String> {
            for (i, e) in events.iter().enumerate() {
                let at = format!("{where_}[{i}]");
                if e.nodes.is_some() && e.count.is_some() {
                    return Err(format!("{at}: `nodes` and `count` both say how many; pick one"));
                }
                if root && e.delay.is_some() {
                    return Err(format!("{at}: `delay` is relative to a parent, and this is a root"));
                }
                if root && e.at_frac.is_none() {
                    return Err(format!("{at}: a root needs `at_frac`"));
                }
                if !root && e.at_frac.is_some() {
                    return Err(format!(
                        "{at}: `at_frac` places a root; a child is placed by `delay` from its parent"
                    ));
                }
                if root && e.nodes.is_none() && !e.then.is_empty() {
                    return Err(format!(
                        "{at}: this root binds no process, so its children have nothing to inherit"
                    ));
                }
                if e.count.is_some() && !e.then.is_empty() {
                    return Err(format!(
                        "{at}: `count` binds no process, so its children have nothing to inherit"
                    ));
                }
                walk(&e.then, false, &at)?;
            }
            Ok(())
        }
        walk(&self.events, true, "events")
    }
}

/// A fingerprint of the space a scenario was drawn from, and of the draw itself.
///
/// Editing a space repoints every seed: seed 47 stops meaning the run it meant yesterday, and
/// nothing fails to say so. Carrying this makes that loud.
pub fn fingerprint(space: &str) -> String {
    let mut h: u64 = 0xCBF2_9CE4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x1000_0000_01B3);
        }
    };
    eat(&DRAW_VERSION.to_le_bytes());
    // Canonical bytes, so reformatting a space file is not mistaken for changing it.
    let text = match serde_json::from_str::<Value>(space) {
        Ok(v) => canonical(&v).to_string(),
        Err(_) => space.to_string(),
    };
    eat(text.as_bytes());
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
    pub space: String,
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
    /// Longest gap between two sends of one process; see [`Space::emit_gap`].
    #[serde(default)]
    pub emit_gap: u64,
    #[serde(default)]
    pub faults: Vec<Fault>,
    #[serde(default)]
    pub stimuli: Vec<Stimulus>,
}


impl Scenario {
    /// Draw a scenario from a seed and a space.
    ///
    /// The draw order is the depth-first walk of the event tree, and within an event: subject,
    /// then placement, then the body's `$rand`s, then children. It is fixed here and never by the
    /// order of keys in a file. Inserting a sibling repoints every seed after it; that is inherent,
    /// and [`fingerprint`] is what makes it loud.
    pub fn draw(seed: u64, space: &Space) -> Scenario {
        let mut r = Rng::new(seed);

        // Capped so the subset mask a partition draws stays inside a u64. Far above any real group.
        let f = space.f.draw(&mut r).min(20);
        let n = (3 * f + 1) as usize;
        let names: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();

        let time_limit = space.time_limit.draw(&mut r).max(2);
        let gst = ((space.gst_frac.draw(&mut r) * time_limit as f64) as u64).min(time_limit);

        let mut delay_pre = vec![vec![0u64; n]; n];
        let mut delay_post = vec![vec![0u64; n]; n];
        for i in 0..n {
            for j in 0..n {
                if i == j {
                    continue;
                }
                delay_pre[i][j] = space.link_delay_pre.draw(&mut r).max(1);
                delay_post[i][j] = space.link_delay_post.draw(&mut r).max(1);
            }
        }

        let jitter_pct = space.jitter_pct.draw(&mut r);
        let emit_gap = space.emit_gap.draw(&mut r);
        let fifo = space.fifo.draw(&mut r);

        let mut d = Draw { r: &mut r, f, n, names: &names, time_limit, faults: vec![], stimuli: vec![] };
        for ev in &space.events {
            d.visit(ev, None, &[]);
        }
        let (mut faults, mut stimuli) = (d.faults, d.stimuli);
        faults.sort_by_key(|f| f.at());
        stimuli.sort_by_key(|s| s.at);

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
            emit_gap,
            faults,
            stimuli,
        }
    }
}

/// The state a walk of the event tree carries.
struct Draw<'a> {
    r: &'a mut Rng,
    f: u64,
    n: usize,
    names: &'a [String],
    time_limit: u64,
    faults: Vec<Fault>,
    stimuli: Vec<Stimulus>,
}

impl Draw<'_> {
    /// Instantiate one event and its subtree.
    ///
    /// `parent` carries the instant to place against and the process to inherit; `taken` carries
    /// the processes already bound on the path, which is what makes `distinct` mean "someone other
    /// than mine" under a parent and "distinct from each other" at the root.
    fn visit(&mut self, ev: &Event, parent: Option<(u64, Option<usize>)>, taken: &[usize]) {
        let subjects: Vec<Option<usize>> = match (&ev.nodes, &ev.count) {
            (Some(sub), _) => self.pick(sub, taken).into_iter().map(Some).collect(),
            (None, Some(c)) => vec![None; c.draw(self.r, self.f) as usize],
            (None, None) => vec![parent.and_then(|(_, who)| who)],
        };

        for (i, who) in subjects.into_iter().enumerate() {
            let at = match parent {
                None => {
                    let frac = ev.at_frac.as_ref().map(|s| s.draw(self.r)).unwrap_or(0.0);
                    (frac * self.time_limit as f64) as u64
                }
                Some((pt, _)) => pt.saturating_add(ev.delay.map(|s| s.draw(self.r)).unwrap_or(0)),
            };
            self.emit(&ev.effect, at, who, i as u64);

            if !ev.then.is_empty() {
                let mut deeper = taken.to_vec();
                if let Some(w) = who {
                    deeper.push(w);
                }
                for child in &ev.then {
                    self.visit(child, Some((at, who)), &deeper);
                }
            }
        }
    }

    /// The processes an event acts on.
    ///
    /// `distinct` uses a partial Fisher-Yates shuffle rather than rejection sampling: rejection
    /// consumes a number of draws that depends on collisions, hence on `n`, and the stream would
    /// shift unpredictably. This consumes exactly k.
    fn pick(&mut self, sub: &Subject, taken: &[usize]) -> Vec<usize> {
        match sub {
            Subject::All => (0..self.n).collect(),
            Subject::Any(k) => {
                let k = k.draw(self.r, self.f) as usize;
                (0..k).map(|_| self.r.range(0, self.n as u64) as usize).collect()
            }
            Subject::Distinct(k) => {
                let want = k.draw(self.r, self.f) as usize;
                let mut pool: Vec<usize> = (0..self.n).filter(|i| !taken.contains(i)).collect();
                let k = want.min(pool.len());
                for i in 0..k {
                    let j = i + self.r.range(0, (pool.len() - i) as u64) as usize;
                    pool.swap(i, j);
                }
                pool.truncate(k);
                pool
            }
        }
    }

    /// Every value an effect needs is drawn before anything is skipped, so an event bound to no
    /// process does not shift the rest of the stream.
    fn emit(&mut self, eff: &Effect, at: u64, who: Option<usize>, i: u64) {
        match eff {
            Effect::Stimulus(body) => {
                let body = render(body, i, self.r);
                if let Some(w) = who {
                    self.stimuli.push(Stimulus { at, node: self.names[w].clone(), body });
                }
            }
            Effect::Crash => {
                if let Some(w) = who {
                    let name = self.names[w].clone();
                    let already = self
                        .faults
                        .iter()
                        .any(|f| matches!(f, Fault::Crash { node, .. } if *node == name));
                    if !already {
                        self.faults.push(Fault::Crash { at, node: name });
                    }
                }
            }
            Effect::Pause { duration } => {
                let d = duration.draw(self.r).max(1);
                if let Some(w) = who {
                    self.faults.push(Fault::Pause { at, node: self.names[w].clone(), duration: d });
                }
            }
            Effect::Partition { duration } => {
                let d = duration.draw(self.r).max(1);
                // Two sides need two processes: a proper non-empty subset of a group of one has no
                // answer. Taking a prefix instead of a drawn subset would put n0 on the small side
                // of every partition ever drawn.
                if self.n >= 2 {
                    let mask = self.r.range(1, (1u64 << self.n) - 1);
                    let side = (0..self.n)
                        .filter(|i| mask >> i & 1 == 1)
                        .map(|i| self.names[i].clone())
                        .collect();
                    self.faults.push(Fault::Partition { at, duration: d, side });
                }
            }
        }
    }
}

impl Scenario {
    /// Record where this was drawn from. Kept off [`Scenario::draw`] so the draw stays a function
    /// of the spaces alone, not of where they happened to be stored.
    pub fn from(mut self, d: Drawn) -> Scenario {
        self.drawn = Some(d);
        self
    }

    /// Delay for the `idx`-th message on link `from -> to`, sent at `now`.
    ///
    /// Jitter is hashed from `(from, to, idx)` rather than drawn from the run's PRNG, so code under
    /// test changing how many messages it sends does not shift every other link's timing.
    /// How long after its step a process gets its `rank`-th send out.
    ///
    /// Hashed from `(from, step, rank)` rather than drawn from the run's PRNG, for the same reason
    /// as the jitter below: code under test that changes how many messages it sends must not shift
    /// every other timing in the run.
    pub fn emit_at(&self, from: usize, step: u64, rank: u64) -> u64 {
        if self.emit_gap == 0 || rank == 0 {
            return 0;
        }
        // The sum of the gaps before it, not `rank` times its own: a send loop is sequential, so
        // the k-th send cannot leave before the (k-1)-th, and only a running total guarantees that.
        (1..=rank)
            .map(|i| {
                let mut h = (from as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(step.wrapping_mul(0xC2B2_AE3D_27D4_EB4F))
                    .wrapping_add(i.wrapping_mul(0x1656_67B1_9E37_79F9));
                h ^= h >> 29;
                h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
                h ^= h >> 32;
                h % (self.emit_gap + 1)
            })
            .sum()
    }

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

    fn space(src: &str) -> Space {
        Space::from_json(src).expect("space parses")
    }
    fn nodes_of(sc: &Scenario) -> Vec<&str> {
        sc.stimuli.iter().map(|s| s.node.as_str()).collect()
    }

    /// A golden test on the one thing that must never drift silently. Seeds address stored runs;
    /// if the draw changes, all of them quietly start meaning something else. A failure here is
    /// either a regression or a deliberate break, announced by bumping DRAW_VERSION.
    #[test]
    fn the_draw_is_pinned() {
        let sp = space(r#"{"f": 1, "fifo": true,
            "events": [{"nodes": {"distinct": [0, "f"]}, "at_frac": [0.0, 0.7], "crash": {}}]}"#);
        let sc = Scenario::draw(3, &sp);
        assert_eq!((sc.nodes, sc.f), (4, 1));
        assert_eq!(sc.gst, 2855);
        assert_eq!(sc.emit_gap, 6);
        assert_eq!(sc.delay_pre[0], vec![0, 362, 48, 136]);
        // This seed draws zero crashes from `[0, f]`.
        assert!(sc.faults.is_empty(), "{:?}", sc.faults);
    }

    // ------------------------------------------------------------ the subject

    #[test]
    fn all_selects_every_process_once_in_order() {
        let sp = space(r#"{"f": 1, "events":
            [{"nodes": "all", "at_frac": 0.1, "stimulus": {"type": "x"}}]}"#);
        assert_eq!(nodes_of(&Scenario::draw(1, &sp)), vec!["n0", "n1", "n2", "n3"]);
    }

    #[test]
    fn distinct_never_repeats_and_any_may() {
        let d = space(r#"{"f": 3, "events":
            [{"nodes": {"distinct": [10, 10]}, "at_frac": 0.1, "stimulus": {"type": "x"}}]}"#);
        for seed in 1..40 {
            let sc = Scenario::draw(seed, &d);
            let got = nodes_of(&sc);
            let mut uniq = got.clone();
            uniq.sort();
            uniq.dedup();
            assert_eq!(uniq.len(), got.len(), "seed {seed}: distinct repeated a process");
        }
        // With replacement, ten draws from ten processes repeat somewhere almost always.
        let a = space(r#"{"f": 3, "events":
            [{"nodes": {"any": [10, 10]}, "at_frac": 0.1, "stimulus": {"type": "x"}}]}"#);
        let repeated = (1..40).filter(|s| {
            let sc = Scenario::draw(*s, &a);
            let got = nodes_of(&sc);
            let mut u = got.clone();
            u.sort();
            u.dedup();
            u.len() < got.len()
        });
        assert!(repeated.count() > 30, "`any` behaved like `distinct`");
    }

    #[test]
    fn distinct_asked_for_more_than_exist_gives_what_exists() {
        let sp = space(r#"{"f": 1, "events":
            [{"nodes": {"distinct": [99, 99]}, "at_frac": 0.1, "stimulus": {"type": "x"}}]}"#);
        assert_eq!(Scenario::draw(1, &sp).stimuli.len(), 4);
    }

    #[test]
    fn the_f_symbol_is_the_fault_budget() {
        let sp = space(r#"{"f": [1, 3], "events":
            [{"nodes": {"distinct": ["f", "f"]}, "at_frac": 0.1, "crash": {}}]}"#);
        for seed in 1..60 {
            let sc = Scenario::draw(seed, &sp);
            assert_eq!(sc.faults.len(), sc.f, "seed {seed}: f crashes expected");
        }
    }

    // --------------------------------------------------------------- the tree

    #[test]
    fn a_child_acts_on_its_parents_process() {
        // The whole reason the language has no variables: the tree carries the binding.
        let sp = space(r#"{"f": 1, "events": [
            {"nodes": {"distinct": 1}, "at_frac": 0.1, "stimulus": {"type": "send"},
             "then": [{"delay": [5, 5], "crash": {}}]}]}"#);
        for seed in 1..40 {
            let sc = Scenario::draw(seed, &sp);
            let who = sc.stimuli[0].node.clone();
            match &sc.faults[0] {
                Fault::Crash { at, node } => {
                    assert_eq!(*node, who, "seed {seed}: the crash left its parent's process");
                    assert_eq!(*at, sc.stimuli[0].at + 5);
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_child_naming_its_own_subject_leaves_the_parents() {
        // `distinct` under a parent means "other than mine": one process broadcasts, another acts.
        let sp = space(r#"{"f": 1, "events": [
            {"nodes": {"distinct": 1}, "at_frac": 0.1, "stimulus": {"type": "a"},
             "then": [{"nodes": {"distinct": 1}, "delay": [1, 9], "stimulus": {"type": "b"}}]}]}"#);
        for seed in 1..40 {
            let sc = Scenario::draw(seed, &sp);
            let a = sc.stimuli.iter().find(|s| s.body["type"] == "a").unwrap();
            let b = sc.stimuli.iter().find(|s| s.body["type"] == "b").unwrap();
            assert_ne!(a.node, b.node, "seed {seed}");
            assert!((1..=9).contains(&(b.at - a.at)));
        }
    }

    #[test]
    fn a_zero_delay_is_the_same_logical_instant() {
        // This is `together`, and it is not a construct: it is a delay of zero.
        let sp = space(r#"{"f": 1, "events": [
            {"nodes": "all", "at_frac": 0.1, "stimulus": {"type": "a"},
             "then": [{"delay": 0, "stimulus": {"type": "b"}}]}]}"#);
        let sc = Scenario::draw(1, &sp);
        assert_eq!(sc.stimuli.len(), 8);
        assert!(sc.stimuli.iter().all(|s| s.at == sc.stimuli[0].at));
    }

    #[test]
    fn the_tree_multiplies() {
        // A subtree is instantiated once per selected process, which is what makes inheritance
        // work and what makes a deep tree unreadable.
        let sp = space(r#"{"f": 1, "events": [
            {"nodes": {"distinct": [3, 3]}, "at_frac": 0.1, "stimulus": {"type": "a"},
             "then": [{"nodes": {"any": [2, 2]}, "delay": 1, "stimulus": {"type": "b"}}]}]}"#);
        let sc = Scenario::draw(1, &sp);
        assert_eq!(sc.stimuli.iter().filter(|s| s.body["type"] == "a").count(), 3);
        assert_eq!(sc.stimuli.iter().filter(|s| s.body["type"] == "b").count(), 6);
    }

    #[test]
    fn count_repeats_an_event_that_binds_nobody() {
        let sp = space(r#"{"f": 1, "events":
            [{"count": [3, 3], "at_frac": [0.1, 0.5], "partition": {"duration": [10, 20]}}]}"#);
        assert_eq!(Scenario::draw(1, &sp).faults.len(), 3);
    }

    // -------------------------------------------------------------- the world

    // --------------------------------------------------- getting messages out

    /// A process does not hand every message to the network at once, and that matters: a crash
    /// inside a send loop is what leaves a broadcast half-delivered, which is the whole reason
    /// reliable broadcast exists.
    #[test]
    fn sends_leave_one_after_another() {
        let sc = Scenario::draw(1, &space(r#"{"f": 2, "emit_gap": [4, 4]}"#));
        assert_eq!(sc.emit_gap, 4);
        let out: Vec<u64> = (0..6).map(|rank| sc.emit_at(0, 1, rank)).collect();
        assert_eq!(out[0], 0, "the first send leaves at once");
        assert!(out.windows(2).all(|w| w[0] <= w[1]), "not cumulative: {out:?}");
        assert!(*out.last().unwrap() > 0, "every send left together: {out:?}");
    }

    #[test]
    fn a_pinned_gap_of_zero_puts_every_send_at_the_same_instant() {
        // Which is what the simulator did before this existed, and why a partial broadcast could
        // not be expressed at all.
        let sc = Scenario::draw(1, &space(r#"{"f": 1, "emit_gap": 0}"#));
        assert!((0..8).all(|r| sc.emit_at(0, 1, r) == 0));
    }

    /// Hashed, not drawn: code under test that sends a different number of messages must not shift
    /// every other timing in the run. The same rule the jitter follows.
    #[test]
    fn the_stagger_does_not_consume_the_run_s_randomness() {
        let sc = Scenario::draw(4, &space(r#"{"f": 2, "emit_gap": [1, 9]}"#));
        // Same inputs, same answer, however often it is asked.
        for _ in 0..3 {
            assert_eq!(sc.emit_at(1, 7, 3), sc.emit_at(1, 7, 3));
        }
        // And a different step of the same node is independent, so a node that takes one step more
        // does not move what its neighbours do.
        let a: Vec<u64> = (0..5).map(|r| sc.emit_at(1, 7, r)).collect();
        let b: Vec<u64> = (0..5).map(|r| sc.emit_at(1, 8, r)).collect();
        assert_ne!(a, b, "two steps staggered identically: {a:?}");
    }

    #[test]
    fn every_stagger_stays_within_the_declared_gap() {
        let sc = Scenario::draw(9, &space(r#"{"f": 3, "emit_gap": [0, 7]}"#));
        for node in 0..4 {
            for step in 1..6 {
                for rank in 1..8u64 {
                    let d = sc.emit_at(node, step, rank);
                    assert!(d <= rank * sc.emit_gap, "rank {rank} waited {d} for gap {}", sc.emit_gap);
                }
            }
        }
    }

    #[test]
    fn a_scalar_and_a_range_of_width_zero_are_the_same_run() {
        let e = r#"{"f": F, "events": [{"nodes": {"distinct": [0, "f"]}, "at_frac": 0.3, "crash": {}}]}"#;
        for seed in 1..30 {
            let a = Scenario::draw(seed, &space(&e.replace("F", "2")));
            let b = Scenario::draw(seed, &space(&e.replace("F", "[2, 2]")));
            assert_eq!(a.to_json(), b.to_json(), "seed {seed}");
        }
    }

    #[test]
    fn changing_a_pinned_field_changes_that_field_and_nothing_else() {
        let base = r#"{"f": 2, "jitter_pct": J, "fifo": B, "events":
            [{"nodes": {"distinct": [0, "f"]}, "at_frac": [0.0, 0.7], "crash": {}}]}"#;
        let quiet = space(&base.replace("J", "100").replace("B", "false"));
        let loud = space(&base.replace("J", "900").replace("B", "true"));
        for seed in 1..30 {
            let a = Scenario::draw(seed, &quiet);
            let b = Scenario::draw(seed, &loud);
            assert_eq!((a.jitter_pct, a.fifo), (100, false));
            assert_eq!((b.jitter_pct, b.fifo), (900, true));
            assert_eq!((a.gst, &a.delay_pre), (b.gst, &b.delay_pre), "seed {seed}");
            assert_eq!(format!("{:?}", a.faults), format!("{:?}", b.faults), "seed {seed}");
        }
    }

    #[test]
    fn the_group_size_follows_the_fault_budget() {
        for f in 1..=4u64 {
            let sc = Scenario::draw(7, &space(&format!(r#"{{"f": {f}}}"#)));
            assert_eq!((sc.nodes, sc.f), ((3 * f + 1) as usize, f as usize));
        }
    }

    #[test]
    fn integer_ranges_are_inclusive() {
        let sizes: Vec<usize> =
            (1..80).map(|s| Scenario::draw(s, &space(r#"{"f": [1, 3]}"#)).nodes).collect();
        assert!(sizes.contains(&4) && sizes.contains(&7) && sizes.contains(&10));
        assert!(sizes.iter().all(|n| [4, 7, 10].contains(n)));
    }

    #[test]
    fn a_partition_is_a_proper_non_empty_subset_and_not_always_a_prefix() {
        let sp = space(r#"{"f": 3, "events":
            [{"count": 1, "at_frac": [0.1, 0.5], "partition": {"duration": [10, 20]}}]}"#);
        let mut non_prefix = 0;
        for seed in 1..120 {
            let sc = Scenario::draw(seed, &sp);
            for f in &sc.faults {
                let Fault::Partition { side, .. } = f else { panic!("{f:?}") };
                assert!(!side.is_empty() && side.len() < sc.nodes, "seed {seed}: {side:?}");
                let prefix: Vec<String> = (0..side.len()).map(|i| format!("n{i}")).collect();
                if *side != prefix {
                    non_prefix += 1;
                }
            }
        }
        assert!(non_prefix > 60, "only {non_prefix} partitions were not a prefix");
    }

    #[test]
    fn a_group_of_one_process_cannot_be_partitioned() {
        let sp = space(r#"{"f": 0, "events":
            [{"count": 3, "at_frac": 0.1, "partition": {"duration": 10}}]}"#);
        let sc = Scenario::draw(1, &sp);
        assert_eq!(sc.nodes, 1);
        assert!(sc.faults.is_empty());
    }

    #[test]
    fn no_events_means_no_stimuli_and_no_faults() {
        let sc = Scenario::draw(1, &space(r#"{"f": 1}"#));
        assert!(sc.stimuli.is_empty() && sc.faults.is_empty());
    }

    // ----------------------------------------------------------- what is refused

    #[test]
    fn a_space_that_cannot_mean_what_it_says_is_refused() {
        let cases = [
            (r#"{"events": [{"nodes": "all", "count": 1, "at_frac": 0.1, "crash": {}}]}"#,
             "pick one"),
            (r#"{"events": [{"nodes": "all", "delay": 1, "crash": {}}]}"#,
             "this is a root"),
            (r#"{"events": [{"nodes": "all", "crash": {}}]}"#, "a root needs `at_frac`"),
            (r#"{"events": [{"nodes": "all", "at_frac": 0.1, "crash": {},
                 "then": [{"at_frac": 0.2, "crash": {}}]}]}"#, "places a root"),
            (r#"{"events": [{"count": 1, "at_frac": 0.1, "partition": {"duration": 1},
                 "then": [{"delay": 1, "crash": {}}]}]}"#, "nothing to inherit"),
            (r#"{"link_delay_pre": [400, 1]}"#, "runs backwards"),
            (r#"{"events": [{"nodes": {"distinct": "g"}, "at_frac": 0.1, "crash": {}}]}"#,
             "an integer or"),
        ];
        for (src, needle) in cases {
            let err = Space::from_json(src).expect_err(&format!("{src} must be refused"));
            assert!(err.contains(needle), "{err:?} does not mention {needle:?}");
        }
    }

    #[test]
    fn an_unknown_field_is_a_mistake_not_a_silence() {
        assert!(Space::from_json(r#"{"crashes": {}}"#).is_err());
        assert!(Space::from_json(r#"{"events": [{"nodez": "all"}]}"#).is_err());
    }

    // ------------------------------------------------------------- provenance

    #[test]
    fn the_fingerprint_follows_meaning_not_formatting() {
        let a = r#"{"f": 1, "fifo": true}"#;
        let reformatted = "{\n  \"fifo\"  : true,\n  \"f\": 1\n}";
        let changed = r#"{"f": 2, "fifo": true}"#;
        assert_eq!(fingerprint(a), fingerprint(reformatted));
        assert_ne!(fingerprint(a), fingerprint(changed));
    }

    #[test]
    fn a_hand_written_scenario_needs_almost_nothing_and_has_no_provenance() {
        let sc = Scenario::from_json(
            r#"{"nodes": 4, "stimuli": [{"at": 10, "node": "n0", "body": {"type": "x"}}]}"#,
        )
        .unwrap();
        assert_eq!((sc.nodes, sc.time_limit, sc.stimuli.len()), (4, 10_000, 1));
        assert!(sc.delay_pre.is_empty() && sc.drawn.is_none());
        assert_eq!(sc.seed(), 0);
    }

    #[test]
    fn provenance_survives_a_round_trip() {
        let src = r#"{"f": 1}"#;
        let sc = Scenario::draw(3, &space(src)).from(Drawn {
            seed: 3,
            space: "spaces/clean.json".into(),
            fingerprint: fingerprint(src),
        });
        let back = Scenario::from_json(&sc.to_json()).expect("round trip");
        assert_eq!(back.seed(), 3);
        let d = back.drawn.expect("provenance kept");
        assert_eq!((d.space.as_str(), d.fingerprint), ("spaces/clean.json", fingerprint(src)));
    }

    // ------------------------------------------------------------- the sweep

    const SPACES: &[&str] = &[
        r#"{}"#,
        r#"{"f": 0, "events": [{"count": 2, "at_frac": 0.1, "partition": {"duration": 10}}]}"#,
        r#"{"f": [0, 3], "events": [
            {"nodes": {"distinct": [0, "f"]}, "at_frac": [0.0, 0.7], "crash": {}},
            {"nodes": {"distinct": [0, 3]}, "at_frac": [0.0, 0.6], "pause": {"duration": [10, 400]}},
            {"count": [0, 2], "at_frac": [0.0, 0.6], "partition": {"duration": [50, 600]}}]}"#,
        r#"{"f": [1, 2], "time_limit": [2000, 9000], "gst_frac": [0.0, 1.0], "events": [
            {"nodes": "all", "at_frac": [0.0, 0.5], "stimulus": {"type": "x", "v": {"$rand": [0, 9]}},
             "then": [{"delay": [0, 40], "stimulus": {"type": "y"}},
                      {"nodes": {"distinct": 1}, "delay": [1, 20], "crash": {}}]}]}"#,
        r#"{"f": 1, "jitter_pct": [0, 300], "fifo": [false, true], "events": [
            {"nodes": {"any": [0, 6]}, "at_frac": [0.0, 0.9], "stimulus": {"type": "z", "id": "m<i>"}}]}"#,
    ];

    /// Every invariant the simulator and any checker are entitled to assume, over every branch.
    #[test]
    fn the_draw_holds_its_invariants_everywhere() {
        for (si, src) in SPACES.iter().enumerate() {
            let sp = space(src);
            for seed in 1..40u64 {
                let w = format!("space {si}, seed {seed}");
                let sc = Scenario::draw(seed, &sp);
                let names: Vec<String> = (0..sc.nodes).map(|i| format!("n{i}")).collect();

                assert_eq!(sc.nodes, 3 * sc.f + 1, "{w}");
                assert!(sc.time_limit >= 2 && sc.gst <= sc.time_limit, "{w}");
                assert_eq!(sc.delay_pre.len(), sc.nodes, "{w}");
                for i in 0..sc.nodes {
                    for j in 0..sc.nodes {
                        let (a, b) = (sc.delay_pre[i][j], sc.delay_post[i][j]);
                        if i == j {
                            assert_eq!((a, b), (0, 0), "{w}");
                        } else {
                            assert!(a >= 1 && b >= 1, "{w}: zero delay {i}->{j}");
                        }
                    }
                }

                let mut crashed: Vec<&str> = vec![];
                for f in &sc.faults {
                    match f {
                        Fault::Crash { node, .. } => {
                            assert!(names.contains(node), "{w}: crash on {node}");
                            assert!(!crashed.contains(&node.as_str()), "{w}: crashed twice");
                            crashed.push(node);
                        }
                        Fault::Pause { node, duration, .. } => {
                            assert!(names.contains(node) && *duration >= 1, "{w}");
                        }
                        Fault::Partition { duration, side, .. } => {
                            assert!(!side.is_empty() && side.len() < sc.nodes, "{w}: {side:?}");
                            assert!(side.iter().all(|s| names.contains(s)) && *duration >= 1, "{w}");
                            let mut seen = side.clone();
                            seen.sort();
                            seen.dedup();
                            assert_eq!(seen.len(), side.len(), "{w}: a process listed twice");
                        }
                    }
                }
                // Le budget `f` n'est plus garanti par la structure : deux branches peuvent faire
                // tomber deux processus différents. C'est à l'auteur de l'espace de le respecter.
                // Ce que le tirage garantit, c'est qu'aucun processus ne meurt deux fois.
                assert!(sc.faults.windows(2).all(|p| p[0].at() <= p[1].at()), "{w}: unsorted");
                assert!(sc.stimuli.windows(2).all(|p| p[0].at <= p[1].at), "{w}: unsorted");
                assert!(sc.stimuli.iter().all(|s| names.contains(&s.node)), "{w}");

                assert_eq!(sc.to_json(), Scenario::draw(seed, &sp).to_json(), "{w}: not a function");
            }
        }
    }

    #[test]
    fn keys_reordered_in_a_body_change_nothing() {
        let one = space(r#"{"f": 1, "events": [{"nodes": "all", "at_frac": 0.1,
            "stimulus": {"a": {"$rand": [0, 999]}, "b": {"$rand": [0, 999]}}}]}"#);
        let other = space(r#"{"f": 1, "events": [{"nodes": "all", "at_frac": 0.1,
            "stimulus": {"b": {"$rand": [0, 999]}, "a": {"$rand": [0, 999]}}}]}"#);
        let vals = |sc: &Scenario| -> Vec<(u64, u64)> {
            sc.stimuli
                .iter()
                .map(|s| (s.body["a"].as_u64().unwrap(), s.body["b"].as_u64().unwrap()))
                .collect()
        };
        assert_eq!(vals(&Scenario::draw(1, &one)), vals(&Scenario::draw(1, &other)));
    }
}
