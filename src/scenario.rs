//! Scenarios: the replay unit.
//!
//! Deliberately *not* "a seed". Student code changes between runs, so a seed would be consumed
//! differently and the same seed would replay a different run. A scenario pins everything the
//! harness controls — GST, per-link delays, the fault schedule — so it replays against any version
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

/// Which workload to generate when expanding a seed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Workload {
    None,
    Broadcast,
    Mutex,
    Consensus,
}

impl Workload {
    pub fn parse(s: &str) -> Result<Workload, String> {
        match s {
            "none" => Ok(Workload::None),
            "broadcast" => Ok(Workload::Broadcast),
            "mutex" => Ok(Workload::Mutex),
            "consensus" => Ok(Workload::Consensus),
            o => Err(format!("unknown workload {o} (none|broadcast|mutex|consensus)")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Scenario {
    /// Seed this was expanded from, for provenance only; replay uses the fields below.
    pub seed: u64,
    pub nodes: usize,
    pub f: usize,
    /// Global Stabilisation Time: delays are bounded only after this.
    pub gst: u64,
    pub time_limit: u64,
    /// Per-link FIFO ordering. Lamport's mutex needs it; Ricart-Agrawala does not.
    pub fifo: bool,
    /// `delay_pre[from][to]`, used for messages sent before GST.
    pub delay_pre: Vec<Vec<u64>>,
    pub delay_post: Vec<Vec<u64>>,
    /// Extra delay per message, as a percentage of that link's base, hashed from
    /// `(link, index-on-link)` so reordering happens without consuming the global PRNG stream.
    ///
    /// Must scale with the base: a flat jitter smaller than the gap between two messages on a link
    /// can never reorder them, which would silently make `fifo` a no-op.
    pub jitter_pct: u64,
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
    pub workload: Workload,
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
            // At most f crashes — the model's whole premise.
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
            delay_pre,
            delay_post,
            jitter_pct: 100,
            faults,
            stimuli: Self::workload(&mut r, o, &names),
        }
    }

    fn workload(r: &mut Rng, o: &ExpandOpts, names: &[String]) -> Vec<Stimulus> {
        let n = o.nodes as u64;
        let mut out: Vec<Stimulus> = Vec::new();
        match o.workload {
            Workload::None => {}
            Workload::Broadcast => {
                for i in 0..r.range(3, 9) {
                    let who = r.range(0, n) as usize;
                    out.push(Stimulus {
                        at: r.range(0, o.time_limit / 2),
                        node: names[who].clone(),
                        body: json!({ "type": "do_broadcast", "mid": format!("m{i}") }),
                    });
                }
            }
            Workload::Mutex => {
                for _ in 0..r.range(4, 12) {
                    let who = r.range(0, n) as usize;
                    out.push(Stimulus {
                        at: r.range(0, o.time_limit / 2),
                        node: names[who].clone(),
                        body: json!({ "type": "request_cs" }),
                    });
                }
            }
            Workload::Consensus => {
                // Every node gets an input at t=0; binary, which is the classic hard case.
                for name in names {
                    out.push(Stimulus {
                        at: 0,
                        node: name.clone(),
                        body: json!({ "type": "propose", "value": r.range(0, 2) }),
                    });
                }
            }
        }
        out.sort_by_key(|s| s.at);
        out
    }

    pub fn node_index(&self, name: &str) -> Option<usize> {
        name.strip_prefix('n').and_then(|s| s.parse::<usize>().ok()).filter(|i| *i < self.nodes)
    }

    /// Delay for the `idx`-th message on link `from -> to`, sent at `now`.
    ///
    /// Jitter is hashed from `(from, to, idx)` rather than drawn from the run's PRNG, so a student
    /// changing how many messages they send does not shift every other link's timing.
    pub fn delay(&self, from: usize, to: usize, idx: u64, now: u64) -> u64 {
        let base =
            if now < self.gst { self.delay_pre[from][to] } else { self.delay_post[from][to] };
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
