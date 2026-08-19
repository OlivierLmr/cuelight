//! The discrete-event simulator.
//!
//! Strictly sequential: exactly one node runs at a time, and it must emit `done` before logical
//! time may advance. Everything the harness controls is pinned by the scenario, so a run replays
//! exactly.

use crate::journal::Journal;
use crate::node::{Node, NodeError};
use crate::proto::{Envelope, HARNESS};
use crate::rng::Rng;
use crate::scenario::{Fault, Scenario};
use serde_json::json;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::path::PathBuf;
use std::time::Duration;

pub struct Config {
    pub program: Vec<String>,
    pub scenario: Scenario,
    pub run_dir: PathBuf,
    pub watchdog: Duration,
}

#[derive(Debug)]
pub struct Outcome {
    pub ok: bool,
    pub reason: String,
    pub end_time: u64,
    pub events: u64,
}

#[derive(Debug, Clone)]
enum Ev {
    Deliver { from: usize, env: Envelope },
    Timer { node: usize, timer_id: u64 },
    Fault(usize),
    Stimulus(usize),
}

struct Scheduled {
    time: u64,
    tiebreak: u64,
    seq: u64,
    ev: Ev,
}

impl PartialEq for Scheduled {
    fn eq(&self, o: &Self) -> bool { self.cmp(o) == Ordering::Equal }
}
impl Eq for Scheduled {}
impl Ord for Scheduled {
    /// Reversed: `BinaryHeap` is a max-heap and we want the earliest event first.
    ///
    /// `tiebreak` is drawn from the PRNG at insertion, so events landing at the same instant are
    /// ordered by the seed rather than by insertion order — without this a sweep would re-explore
    /// a single interleaving forever. `seq` only breaks exact tiebreak collisions.
    fn cmp(&self, o: &Self) -> Ordering {
        o.time.cmp(&self.time)
            .then_with(|| o.tiebreak.cmp(&self.tiebreak))
            .then_with(|| o.seq.cmp(&self.seq))
    }
}
impl PartialOrd for Scheduled {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> { Some(self.cmp(o)) }
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
    paused_until: Vec<u64>,
    partition_until: u64,
    partition_side: Vec<bool>,
    link_count: HashMap<(usize, usize), u64>,
    last_sched: HashMap<(usize, usize), u64>,
}

impl Sim {
    pub fn new(cfg: Config) -> Result<Sim, String> {
        std::fs::create_dir_all(&cfg.run_dir).map_err(|e| e.to_string())?;
        std::fs::write(cfg.run_dir.join("scenario.json"), cfg.scenario.to_json())
            .map_err(|e| e.to_string())?;
        let journal =
            Journal::create(&cfg.run_dir.join("journal.jsonl")).map_err(|e| e.to_string())?;

        let n = cfg.scenario.nodes;
        let ids: Vec<String> = (0..n).map(|i| format!("n{i}")).collect();
        let mut nodes = Vec::new();
        let mut index = HashMap::new();
        for (i, id) in ids.iter().enumerate() {
            nodes.push(Node::spawn(id, &cfg.program, &cfg.run_dir).map_err(|e| e.to_string())?);
            index.insert(id.clone(), i);
        }

        let rng = Rng::new(cfg.scenario.seed ^ 0xD1B5_4A32_D192_ED03);
        Ok(Sim {
            rng,
            nodes,
            index,
            queue: BinaryHeap::new(),
            journal,
            now: 0,
            seq: 0,
            paused_until: vec![0; n],
            partition_until: 0,
            partition_side: vec![false; n],
            link_count: HashMap::new(),
            last_sched: HashMap::new(),
            cfg,
        })
    }

    fn schedule(&mut self, time: u64, ev: Ev) {
        let tiebreak = self.rng.next_u64();
        let seq = self.seq;
        self.seq += 1;
        self.queue.push(Scheduled { time, tiebreak, seq, ev });
    }

    fn split_by_partition(&self, a: usize, b: usize) -> bool {
        self.now < self.partition_until && self.partition_side[a] != self.partition_side[b]
    }

    fn step_node(&mut self, idx: usize, event: Envelope) -> Result<(), String> {
        if !self.nodes[idx].alive {
            return Ok(());
        }
        let outputs = match self.nodes[idx].step(&event, self.cfg.watchdog) {
            Ok(o) => o,
            // Only reachable for a node the harness did NOT crash: `kill` clears `alive`, and the
            // guard above skips dead nodes. So this is the student's process exiting on its own —
            // a bug in their code, not an injected fault. Failing the run is the point: silently
            // treating it as a crash would let a program that dies on a parse error pass as
            // "correct under f=1 crashes".
            Err(NodeError::Eof(id)) => {
                self.journal.note(self.now, "node-died", json!({ "node": id }));
                self.nodes[idx].alive = false;
                return Err(format!(
                    "node {id} exited on its own at t={} — the harness did not crash it \
                     (check {}/{id}.stderr)",
                    self.now,
                    self.cfg.run_dir.display()
                ));
            }
            Err(e) => return Err(e.to_string()),
        };

        for env in outputs {
            if env.dest == HARNESS {
                self.handle_harness_message(idx, env);
                continue;
            }
            let Some(&to) = self.index.get(&env.dest) else {
                self.journal.note(self.now, "unknown-destination", json!({ "dest": env.dest }));
                continue;
            };
            self.journal.record(self.now, "send", &env);

            let link = (idx, to);
            let n = self.link_count.entry(link).or_insert(0);
            *n += 1;
            let msg_idx = *n;
            let mut at = self.now + self.cfg.scenario.delay(idx, to, msg_idx, self.now);

            // Per-link FIFO: Lamport's mutex needs it, Ricart-Agrawala does not. Turning it off is
            // a deliberate exercise — a correct Lamport implementation breaks without it.
            if self.cfg.scenario.fifo {
                let prev = *self.last_sched.get(&link).unwrap_or(&0);
                if at <= prev {
                    at = prev + 1;
                }
                self.last_sched.insert(link, at);
            }
            self.schedule(at, Ev::Deliver { from: idx, env });
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

    fn apply_fault(&mut self, i: usize) {
        let f = self.cfg.scenario.faults[i].clone();
        match f {
            Fault::Crash { node, .. } => {
                if let Some(&idx) = self.index.get(&node) {
                    self.journal.note(self.now, "fault-crash", json!({ "node": node }));
                    self.nodes[idx].kill();
                }
            }
            Fault::Pause { node, duration, .. } => {
                if let Some(&idx) = self.index.get(&node) {
                    self.paused_until[idx] = self.now + duration;
                    self.journal.note(
                        self.now,
                        "fault-pause",
                        json!({ "node": node, "until": self.now + duration }),
                    );
                }
            }
            Fault::Partition { duration, side, .. } => {
                for m in self.partition_side.iter_mut() {
                    *m = false;
                }
                for name in &side {
                    if let Some(&idx) = self.index.get(name) {
                        self.partition_side[idx] = true;
                    }
                }
                self.partition_until = self.now + duration;
                self.journal.note(
                    self.now,
                    "fault-partition",
                    json!({ "side": side, "until": self.partition_until }),
                );
            }
        }
    }

    pub fn run(mut self) -> Result<Outcome, String> {
        for (i, f) in self.cfg.scenario.faults.iter().enumerate() {
            let at = f.at();
            let tiebreak = self.rng.next_u64();
            let seq = self.seq;
            self.seq += 1;
            self.queue.push(Scheduled { time: at, tiebreak, seq, ev: Ev::Fault(i) });
        }

        for (i, st) in self.cfg.scenario.stimuli.iter().enumerate() {
            let at = st.at;
            let tiebreak = self.rng.next_u64();
            let seq = self.seq;
            self.seq += 1;
            self.queue.push(Scheduled { time: at, tiebreak, seq, ev: Ev::Stimulus(i) });
        }

        let ids: Vec<String> = self.nodes.iter().map(|n| n.id.clone()).collect();
        for i in 0..self.nodes.len() {
            // Deliberately NOT sent: `gst`. Knowing it lets a node simply refuse to suspect
            // anyone until GST has passed — a perfect failure detector with no adaptive timeout,
            // which passes every seed and skips the whole content of lab 2. It is still in
            // `scenario.json` in the run directory, so it stays available for debugging a trace
            // without being reachable from inside the algorithm.
            let body = json!({
                "type": "init",
                "node_id": ids[i],
                "node_ids": ids,
                "n": self.cfg.scenario.nodes,
                "f": self.cfg.scenario.f,
                "provided": [],
            });
            let env = Envelope::new(HARNESS, &ids[i], body);
            self.journal.record(0, "init", &env);
            self.step_node(i, env)?;
        }

        let mut reason = "quiescent".to_string();
        while let Some(s) = self.queue.pop() {
            if s.time > self.cfg.scenario.time_limit {
                reason = "time-limit".into();
                self.journal.note(
                    self.now,
                    "time-limit",
                    json!({ "limit": self.cfg.scenario.time_limit }),
                );
                break;
            }
            self.now = s.time;

            match s.ev {
                Ev::Fault(i) => self.apply_fault(i),

                Ev::Stimulus(i) => {
                    let st = self.cfg.scenario.stimuli[i].clone();
                    let Some(&idx) = self.index.get(&st.node) else { continue };
                    if self.now < self.paused_until[idx] {
                        let at = self.paused_until[idx];
                        self.schedule(at, Ev::Stimulus(i));
                        continue;
                    }
                    if !self.nodes[idx].alive { continue }
                    let env = Envelope::new(HARNESS, &st.node, st.body);
                    self.journal.record(self.now, "stimulus", &env);
                    self.step_node(idx, env)?;
                }

                Ev::Deliver { from, env } => {
                    let Some(&to) = self.index.get(&env.dest) else { continue };

                    // Held, not dropped: a partition that heals still delivers.
                    if self.split_by_partition(from, to) {
                        let at = self.partition_until;
                        self.schedule(at, Ev::Deliver { from, env });
                        continue;
                    }
                    // A paused node is alive but reacting to nothing yet.
                    if self.now < self.paused_until[to] {
                        let at = self.paused_until[to];
                        self.schedule(at, Ev::Deliver { from, env });
                        continue;
                    }
                    if !self.nodes[to].alive {
                        self.journal.note(
                            self.now,
                            "drop-to-crashed",
                            json!({ "dest": env.dest }),
                        );
                        continue;
                    }
                    self.journal.record(self.now, "recv", &env);
                    self.step_node(to, env)?;
                }

                Ev::Timer { node, timer_id } => {
                    if self.now < self.paused_until[node] {
                        let at = self.paused_until[node];
                        self.schedule(at, Ev::Timer { node, timer_id });
                        continue;
                    }
                    if !self.nodes[node].alive {
                        continue;
                    }
                    let id = self.nodes[node].id.clone();
                    let env =
                        Envelope::new(HARNESS, &id, json!({ "type": "timer", "timer_id": timer_id }));
                    self.journal.record(self.now, "timer", &env);
                    self.step_node(node, env)?;
                }
            }
        }

        self.journal.note(
            self.now,
            "end",
            json!({ "reason": reason, "scheduled": self.seq }),
        );
        let end_time = self.now;
        let events = self.seq;
        self.journal.finish().map_err(|e| e.to_string())?;
        Ok(Outcome { ok: true, reason, end_time, events })
    }
}
