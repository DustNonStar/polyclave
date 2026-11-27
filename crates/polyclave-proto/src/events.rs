use serde::{Deserialize, Serialize};
use serde_json::Value;
use ts_rs::TS;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ChangeTopic {
    Message,
    Turn,
    Ticket,
    Run,
    Attempt,
    Memory,
    HubReceipt,
    Inbox,
    Council,
    PrGate,
    Pause,
    Device,
    MeshNode,
    Notification,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
pub struct SseEnvelope {
    #[ts(type = "number")]
    pub seq: i64,
    pub topic: ChangeTopic,
    pub entity_id: String,
    pub payload: Value,
    pub created_at: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum AgentEvent {
    Session { id: String },
    TextDelta { text: String },
    ReasoningDelta { text: String },
    Finished { success: bool },
}
