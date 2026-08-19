//! Process supervision: spawn a student binary, frame JSON lines over its stdio, and enforce the
//! `done` quiescence barrier.
//!
//! The barrier is what makes the simulator deterministic: exactly one node runs at a time, and it
//! must announce that it has finished reacting before logical time may advance.

use crate::proto::{Envelope, HARNESS};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

pub struct Node {
    pub id: String,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    pub alive: bool,
}

#[derive(Debug)]
pub enum NodeError {
    Spawn(std::io::Error),
    /// The node exited or closed stdout before emitting `done`.
    Eof(String),
    Io(std::io::Error),
    Json(String, String),
}

impl std::fmt::Display for NodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NodeError::Spawn(e) => write!(f, "could not spawn node: {e}"),
            NodeError::Eof(id) => write!(f, "node {id} closed its output before emitting `done`"),
            NodeError::Io(e) => write!(f, "io error talking to node: {e}"),
            NodeError::Json(id, l) => write!(f, "node {id} emitted invalid JSON: {l}"),
        }
    }
}

impl Node {
    pub fn spawn(id: &str, program: &[String], log_dir: &Path) -> Result<Node, NodeError> {
        let stderr = std::fs::File::create(log_dir.join(format!("{id}.stderr")))
            .map_err(NodeError::Spawn)?;
        let mut cmd = Command::new(&program[0]);
        cmd.args(&program[1..])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(stderr));
        let mut child = cmd.spawn().map_err(NodeError::Spawn)?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Ok(Node { id: id.into(), child, stdin, stdout, alive: true })
    }

    /// Deliver one event and collect everything the node emits until its `done`.
    ///
    /// Returns the outgoing envelopes, excluding the terminating `done` itself.
    pub fn step(&mut self, event: &Envelope) -> Result<Vec<Envelope>, NodeError> {
        let line = serde_json::to_string(event).expect("envelope serialises");
        writeln!(self.stdin, "{line}").map_err(NodeError::Io)?;
        self.stdin.flush().map_err(NodeError::Io)?;

        let mut out = Vec::new();
        loop {
            let mut buf = String::new();
            let n = self.stdout.read_line(&mut buf).map_err(NodeError::Io)?;
            if n == 0 {
                return Err(NodeError::Eof(self.id.clone()));
            }
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

    /// Crash the node: it stops forever and never recovers, per the fault model.
    pub fn kill(&mut self) {
        if self.alive {
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
