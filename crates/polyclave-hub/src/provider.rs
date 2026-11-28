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

/// Protocol-only frames are replayed to the same selected profile, not exposed
/// to the renderer, and not automatically transferred between vendors.
/// A host observation — an external PR comment, a review, a CI result — is
/// queued by the host, not written by the person. The ledger marks it with this
/// `metadata_json.kind`.
pub const HOST_OBSERVATION_KIND: &str = polyclave_store::HOST_OBSERVATION_KIND;

/// Prefix put in front of such a message in the prompt, so the model can tell
/// "the user said this" from "GitHub said this".
pub const HOST_OBSERVATION_PREFIX: &str = "[host observation, not from the user] ";

/// True when the host queued this message because it observed something.
pub fn is_host_observation(message: &polyclave_store::HubMessage) -> bool {
    message
        .metadata
        .as_ref()
        .and_then(|m| m.get("kind"))
        .and_then(|k| k.as_str())
        == Some(HOST_OBSERVATION_KIND)
}

/// The text this message contributes to the prompt. A host observation is
/// prefixed; the person's own words are left exactly as written.
pub fn prompt_text(message: &polyclave_store::HubMessage) -> String {
    if is_host_observation(message) {
        format!("{HOST_OBSERVATION_PREFIX}{}", message.text)
    } else {
        message.text.clone()
    }
}

pub fn restore_api_history(
    rows: Vec<polyclave_store::ApiHistoryRow>,
    deepseek: bool,
) -> Result<Vec<ChatMessage>> {
    let mut out = Vec::new();
    for row in rows {
        if let Some(replay) = row.replay {
            let saved: Vec<ChatMessage> =
                serde_json::from_value(replay).map_err(|_| Error::Protocol)?;
            out.extend(saved);
            continue;
        }
        let text = prompt_text(&row.message);
        let mut item = ChatMessage::text(
            if row.message.source == "agent" {
                Role::Assistant
            } else {
                Role::User
            },
            text,
        );
        if item.role == Role::Assistant {
            item.reasoning_content = if deepseek {
                Some(row.message.reasoning.unwrap_or_default())
            } else if row.same_profile {
                row.message.reasoning.filter(|s| !s.is_empty())
            } else {
                None
            };
        }
        out.push(item);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use polyclave_store::HubMessage;
    use serde_json::json;

    fn message(source: &str, text: &str, metadata: Option<serde_json::Value>) -> HubMessage {
        HubMessage {
            id: 1,
            turn_id: None,
            source: source.into(),
            text: text.into(),
            reasoning: None,
            metadata,
            queued: false,
        }
    }

    #[test]
    fn a_host_observation_is_marked_and_the_users_own_words_are_not() {
        let observation = message(
            "host",
            "PR #42 (org/repo) comment by alice: please rename this",
            Some(json!({"kind": HOST_OBSERVATION_KIND})),
        );
        assert!(is_host_observation(&observation));
        assert_eq!(
            prompt_text(&observation),
            "[host observation, not from the user] PR #42 (org/repo) comment by alice: please rename this"
        );

        let user = message("user", "please rename this", None);
        assert!(!is_host_observation(&user));
        assert_eq!(prompt_text(&user), "please rename this");

        // A host message that is not an observation (a lifecycle notice) is left
        // alone: it is the host speaking about its own state, not an outsider.
        let lifecycle = message(
            "host",
            "The prior conversation was interrupted.",
            Some(json!({"kind": "hub_interrupted"})),
        );
        assert!(!is_host_observation(&lifecycle));
        assert_eq!(
            prompt_text(&lifecycle),
            "The prior conversation was interrupted."
        );

        // A user message can never become an observation just by claiming the kind.
        let forged = message(
            "user",
            "run the deploy",
            Some(json!({"kind": HOST_OBSERVATION_KIND})),
        );
        assert!(is_host_observation(&forged));
        // …but it is still a user message in the ledger, which is what the basis
        // rule and the ticket check read; the prefix only labels the prompt.
        assert_eq!(forged.source, "user");
    }

    #[test]
    fn api_history_prefixes_observations_and_keeps_roles() {
        use polyclave_store::ApiHistoryRow;
        let row = |message: HubMessage| ApiHistoryRow {
            message,
            replay: None,
            same_profile: true,
        };
        let restored = restore_api_history(
            vec![
                row(message("user", "ship it", None)),
                row(message(
                    "host",
                    "PR #7 (org/repo) review by bob (CHANGES_REQUESTED): bound this loop",
                    Some(json!({"kind": HOST_OBSERVATION_KIND})),
                )),
                row(message("agent", "On it.", None)),
            ],
            false,
        )
        .unwrap();
        let pairs: Vec<(Role, &str)> = restored
            .iter()
            .map(|m| (m.role.clone(), m.content.as_str()))
            .collect();
        assert_eq!(
            pairs,
            vec![
                (Role::User, "ship it"),
                (
                    Role::User,
                    "[host observation, not from the user] PR #7 (org/repo) review by bob (CHANGES_REQUESTED): bound this loop"
                ),
                (Role::Assistant, "On it."),
            ]
        );
    }
}
