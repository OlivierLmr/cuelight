//! The discrete-event simulator.
//!
//! Strictly sequential: exactly one node runs at a time, and it must emit `done` before logical
//! time may advance. Every source of nondeterminism the harness controls — message delay, and the
//! order of events falling at the same instant — is drawn from the seeded PRNG.

use crate::journal::Journal;
use crate::node::{Node, NodeError};
use crate::proto::{Envelope, HARNESS};
use crate::rng::Rng;
use serde_json::json;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;

pub struct Config {
    pub program: Vec<String>,
    pub node_count: usize,
    pub f: usize,
    pub seed: u64,
    pub min_delay: u64,
    pub max_delay: u64,
    pub time_limit: u64,
    pub run_dir: PathBuf,
}

#[derive(Debug, Clone)]
enum Ev {
    Deliver(Envelope),
    Timer { node: usize, timer_id: u64 },
}

struct Scheduled {
    time: u64,
    tiebreak: u64,
    seq: u64,
    ev: Ev,
}

impl PartialEq for Scheduled {
    fn eq(&self, o: &Self) -> bool {
        self.cmp(o) == Ordering::Equal
    }
}
impl Eq for Scheduled {}
impl Ord for Scheduled {
    /// Reversed: `BinaryHeap` is a max-heap and we want the earliest event first.
    ///
    /// `tiebreak` is drawn from the PRNG at insertion, so events landing at the same logical
    /// instant are ordered by the seed rather than by insertion order. Without this a seed sweep
    /// would re-explore one interleaving forever. `seq` only breaks exact tiebreak collisions.
    fn cmp(&self, o: &Self) -> Ordering {
        o.time
            .cmp(&self.time)
            .then_with(|| o.tiebreak.cmp(&self.tiebreak))
            .then_with(|| o.seq.cmp(&self.seq))
    }
}
impl PartialOrd for Scheduled {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

pub struct Sim {
    cfg: Config,
    rng: Rng,
    nodes: Vec<Node>,
    index: HashMap<String, usize>,
    queue: BinaryHeap<Scheduled>,
    journal: Journal,
    now: u64,
    seq: u64,
}

impl Sim {
    pub fn new(cfg: Config) -> Result<Sim, String> {
        std::fs::create_dir_all(&cfg.run_dir).map_err(|e| e.to_string())?;
        let journal =
            Journal::create(&cfg.run_dir.join("journal.jsonl")).map_err(|e| e.to_string())?;

        let ids: Vec<String> = (0..cfg.node_count).map(|i| format!("n{i}")).collect();
        let mut nodes = Vec::new();
        let mut index = HashMap::new();
        for (i, id) in ids.iter().enumerate() {
            let n = Node::spawn(id, &cfg.program, &cfg.run_dir).map_err(|e| e.to_string())?;
            nodes.push(n);
            index.insert(id.clone(), i);
        }

        let rng = Rng::new(cfg.seed);
        Ok(Sim { cfg, rng, nodes, index, queue: BinaryHeap::new(), journal, now: 0, seq: 0 })
    }

    fn schedule(&mut self, time: u64, ev: Ev) {
        let tiebreak = self.rng.next_u64();
        let seq = self.seq;
        self.seq += 1;
        self.queue.push(Scheduled { time, tiebreak, seq, ev });
    }

    /// Deliver one event to a node and route everything it emits.
    fn step_node(&mut self, idx: usize, event: Envelope) -> Result<(), String> {
        if !self.nodes[idx].alive {
            return Ok(()); // crashed nodes silently swallow everything
        }
        let outputs = match self.nodes[idx].step(&event) {
            Ok(o) => o,
            Err(NodeError::Eof(id)) => {
                self.journal.note(self.now, "node-died", json!({ "node": id }));
                self.nodes[idx].alive = false;
                return Ok(());
            }
            Err(e) => return Err(e.to_string()),
        };

        for env in outputs {
            if env.dest == HARNESS {
                self.handle_harness_message(idx, env);
            } else {
                self.journal.record(self.now, "send", &env);
                let delay = self.rng.range(self.cfg.min_delay, self.cfg.max_delay + 1);
                let at = self.now + delay;
                self.schedule(at, Ev::Deliver(env));
            }
        }
        Ok(())
    }

    fn handle_harness_message(&mut self, idx: usize, env: Envelope) {
        match env.kind() {
            "set_timer" => {
                let after = env.u64_field("after").unwrap_or(0);
                let timer_id = env.u64_field("timer_id").unwrap_or(0);
                self.journal.record(self.now, "set_timer", &env);
                let at = self.now + after;
                self.schedule(at, Ev::Timer { node: idx, timer_id });
            }
            // Observable events the checkers will consume. M0 only records them.
            "deliver" | "enter_cs" | "exit_cs" | "leader" | "view" | "decide" => {
                self.journal.record(self.now, "observe", &env);
            }
            other => {
                self.journal.note(
                    self.now,
                    "unknown-harness-message",
                    json!({ "node": env.src, "type": other }),
                );
            }
        }
    }

    pub fn run(mut self) -> Result<(), String> {
        // Init, in node order, at t = 0.
        let ids: Vec<String> = self.nodes.iter().map(|n| n.id.clone()).collect();
        for i in 0..self.nodes.len() {
            let body = json!({
                "type": "init",
                "node_id": ids[i],
                "node_ids": ids,
                "n": self.cfg.node_count,
                "f": self.cfg.f,
                "provided": [],
            });
            let env = Envelope::new(HARNESS, &ids[i], body);
            self.journal.record(0, "init", &env);
            self.step_node(i, env)?;
        }

        while let Some(s) = self.queue.pop() {
            if s.time > self.cfg.time_limit {
                self.journal.note(self.now, "time-limit", json!({ "limit": self.cfg.time_limit }));
                break;
            }
            self.now = s.time;
            match s.ev {
                Ev::Deliver(env) => {
                    let Some(&idx) = self.index.get(&env.dest) else {
                        self.journal.note(
                            self.now,
                            "unknown-destination",
                            json!({ "dest": env.dest }),
                        );
                        continue;
                    };
                    self.journal.record(self.now, "recv", &env);
                    self.step_node(idx, env)?;
                }
                Ev::Timer { node, timer_id } => {
                    let id = self.nodes[node].id.clone();
                    let env = Envelope::new(
                        HARNESS,
                        &id,
                        json!({ "type": "timer", "timer_id": timer_id }),
                    );
                    self.journal.record(self.now, "timer", &env);
                    self.step_node(node, env)?;
                }
            }
        }

        self.journal.note(self.now, "end", json!({ "scheduled": self.seq }));
        self.journal.finish().map_err(|e| e.to_string())
    }
}
