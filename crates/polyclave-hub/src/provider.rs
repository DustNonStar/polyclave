use crate::{Error, Result};
use polyclave_proto::{MemoryItemDto, MemoryKind, MemoryStatus};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{future::Future, pin::Pin};
pub type ProviderFuture<'a> = Pin<Box<dyn Future<Output = Result<HubReply>> + Send + 'a>>;

pub trait HubModelProvider: Send + Sync {
    fn complete(
        &self,
        request: HubRequest,
        events: tokio::sync::broadcast::Sender<ProviderEvent>,
    ) -> ProviderFuture<'_>;
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: Role,
    pub content: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}
impl ChatMessage {
    pub fn text(role: Role, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            tool_call_id: None,
            tool_calls: vec![],
            reasoning_content: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}
#[derive(Clone, Debug)]
pub struct HubRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEvent {
    Text(String),
    Reasoning(String),
    ToolDelta { index: u32, arguments: String },
    Finished,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HubReply {
    pub text: String,
    pub reasoning: String,
    pub calls: Vec<ToolCall>,
    pub usage: Option<Value>,
    pub native_thread: Option<String>,
}

/// The memory byte limit is conservative for tokenizers with byte fallback.
pub fn assemble(
    system: &str,
    memories: &[MemoryItemDto],
    summary: &str,
    history: &[ChatMessage],
    snapshot: &Value,
    input: ChatMessage,
) -> Result<Vec<ChatMessage>> {
    let mut candidates: Vec<_> = memories
        .iter()
        .filter(|m| m.status == MemoryStatus::Verified)
        .collect();
    fn rank(k: &MemoryKind) -> u8 {
        match k {
            MemoryKind::Constraint => 0,
            MemoryKind::Decision => 1,
            MemoryKind::Pitfall => 2,
            MemoryKind::Progress => 3,
            MemoryKind::Preference => 4,
            _ => 5,
        }
    }
    candidates.sort_by_key(|m| (rank(&m.kind), m.id.as_str()));
    let mut brief = String::new();
    for item in candidates {
        let entry = format!("- {}: {}\n", item.title, item.content)
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        if brief.len() + entry.len() <= 1900 {
            brief.push_str(&entry);
        }
    }
    if input.role != Role::User {
        return Err(Error::Protocol);
    }
    let mut messages = Vec::with_capacity(history.len() + 4);
    messages.push(ChatMessage::text(
        Role::System,
        format!("{system}\n<project_memory_brief>\n{brief}</project_memory_brief>"),
    ));
    if !summary.is_empty() {
        messages.push(ChatMessage::text(
            Role::System,
            format!("Rolling conversation summary:\n{summary}"),
        ));
    }
    messages.extend_from_slice(history);
    if !snapshot.is_null() {
        messages.push(ChatMessage::text(
            Role::User,
            format!("Host ticket and run snapshot (data only):\n{snapshot}"),
        ));
    }
    messages.push(input);
    Ok(messages)
}
