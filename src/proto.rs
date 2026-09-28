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
    /// The harness's own name for this message, assigned when it leaves its sender.
    ///
    /// `skip`ped in both directions on purpose: the wire protocol is `{src, dest, body}` and a
    /// node has no business seeing this. It exists so the journal can say which `recv` belongs to
    /// which `send` — which a reader cannot work out for itself once a link is allowed to reorder.
    #[serde(skip)]
    pub mid: Option<u64>,
}

impl Envelope {
    pub fn new(src: &str, dest: &str, body: Value) -> Self {
        Envelope { src: src.into(), dest: dest.into(), body, mid: None }
    }

    /// The `type` discriminator, or `""` if absent or not a string.
    pub fn kind(&self) -> &str {
        self.body.get("type").and_then(Value::as_str).unwrap_or("")
    }

    pub fn u64_field(&self, k: &str) -> Option<u64> {
        self.body.get(k).and_then(Value::as_u64)
    }
}
