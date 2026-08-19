//! Process supervision: spawn a student binary, frame JSON lines over its stdio, and enforce the
//! `done` quiescence barrier.
//!
//! The barrier is what makes the simulator deterministic: exactly one node runs at a time, and it
//! must announce that it has finished reacting before logical time may advance.
//!
//! ## The watchdog and wall-clock time
//!
//! Reading is delegated to a thread so that a node which hangs (infinite loop, forgotten `done`)
//! can be detected instead of blocking the harness forever. That check uses *wall-clock* time —
//! the only wall-clock in the system. It is sound because it can only ever turn a run that would
//! have hung into a reported failure: a run that completes is unaffected, so determinism of
//! successful runs is preserved.

use crate::proto::{Envelope, HARNESS};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::Duration;

pub struct Node {
    pub id: String,
    child: Child,
    stdin: Option<ChildStdin>,
    rx: Receiver<String>,
    pub alive: bool,
}

#[derive(Debug)]
pub enum NodeError {
    Spawn(std::io::Error),
    /// Exited or closed stdout before emitting `done`.
    Eof(String),
    /// Produced nothing for the watchdog interval — almost always a missing `done` or a loop.
    Hung(String),
    Io(std::io::Error),
    Json(String, String),
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeError::Spawn(e) => write!(f, "could not spawn node: {e}"),
            NodeError::Eof(id) => write!(f, "node {id} closed its output before emitting `done`"),
            NodeError::Hung(id) => write!(
                f,
                "node {id} produced no output within the watchdog interval \
                 (missing `done`, or an infinite loop in a handler)"
            ),
            NodeError::Io(e) => write!(f, "io error talking to node: {e}"),
            NodeError::Json(id, l) => write!(f, "node {id} emitted invalid JSON: {l}"),
        }
    }
}

impl Node {
    pub fn spawn(id: &str, program: &[String], log_dir: &Path) -> Result<Node, NodeError> {
        let stderr = std::fs::File::create(log_dir.join(format!("{id}.stderr")))
            .map_err(NodeError::Spawn)?;
        let mut child = Command::new(&program[0])
            .args(&program[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr))
            .spawn()
            .map_err(NodeError::Spawn)?;

        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut buf = String::new();
                match r.read_line(&mut buf) {
                    Ok(0) | Err(_) => break, // EOF: dropping tx disconnects the channel
                    Ok(_) => {
                        if tx.send(buf).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Node { id: id.into(), child, stdin: Some(stdin), rx, alive: true })
    }

    /// Deliver one event and collect everything the node emits until its `done`.
    pub fn step(&mut self, event: &Envelope, watchdog: Duration) -> Result<Vec<Envelope>, NodeError> {
        let line = serde_json::to_string(event).expect("envelope serialises");
        {
            let w = self.stdin.as_mut().ok_or_else(|| NodeError::Eof(self.id.clone()))?;
            writeln!(w, "{line}").map_err(NodeError::Io)?;
            w.flush().map_err(NodeError::Io)?;
        }

        let mut out = Vec::new();
        loop {
            let buf = match self.rx.recv_timeout(watchdog) {
                Ok(b) => b,
                Err(RecvTimeoutError::Timeout) => return Err(NodeError::Hung(self.id.clone())),
                Err(RecvTimeoutError::Disconnected) => return Err(NodeError::Eof(self.id.clone())),
            };
            let trimmed = buf.trim();
            if trimmed.is_empty() {
                continue;
            }
            let env: Envelope = serde_json::from_str(trimmed)
                .map_err(|_| NodeError::Json(self.id.clone(), trimmed.to_string()))?;
            if env.dest == HARNESS && env.kind() == "done" {
                return Ok(out);
            }
            out.push(env);
        }
    }

    /// Crash: stops forever, never recovers.
    pub fn kill(&mut self) {
        if self.alive {
            self.stdin.take(); // close stdin so the child can exit cleanly
            let _ = self.child.kill();
            let _ = self.child.wait();
            self.alive = false;
        }
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        self.kill();
    }
}
