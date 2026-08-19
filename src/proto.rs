//! The wire protocol.
//!
//! Maelstrom's envelope plus three additive types (`set_timer`, `timer`, `done`). The harness only
//! interprets messages addressed to `HARNESS`; anything addressed to a node is opaque payload it
//! merely routes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const HARNESS: &str = "harness";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope {
    pub src: String,
    pub dest: String,
    pub body: Value,
}

impl Envelope {
    pub fn new(src: &str, dest: &str, body: Value) -> Self {
        Envelope { src: src.into(), dest: dest.into(), body }
    }

    /// The `type` discriminator, or `""` if absent or not a string.
    pub fn kind(&self) -> &str {
        self.body.get("type").and_then(Value::as_str).unwrap_or("")
    }

    pub fn u64_field(&self, k: &str) -> Option<u64> {
        self.body.get(k).and_then(Value::as_u64)
    }
}
