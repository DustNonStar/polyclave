use crate::{
    environment, home, push_option, roots, tmp_config_path, ClaudeIsolation, Error, WorkerCommand,
    WorkerContext,
};
use polyclave_os::{resolve_cli_binary, write_private_file};
use polyclave_proto::AgentEvent;
use serde_json::{json, Value};
use std::{ffi::OsString, path::Path};
use uuid::Uuid;

pub fn claude_args(config: &Path, id: Uuid, resume: bool, model: Option<&str>) -> Vec<OsString> {
    let mut args = [
        "-p",
        "--verbose",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-mode",
        "bypassPermissions",
        "--strict-mcp-config",
        "--setting-sources",
        "",
    ]
    .map(OsString::from)
    .to_vec();
    push_option(&mut args, "--mcp-config", config);
    push_option(
        &mut args,
        if resume { "--resume" } else { "--session-id" },
        id.to_string(),
    );
    if let Some(model) = model {
        push_option(&mut args, "--model", model);
    }
    args
}

pub fn claude_worker(
    ctx: &WorkerContext<'_>,
    new_id: Uuid,
    isolation: Option<&dyn ClaudeIsolation>,
) -> Result<WorkerCommand, Error> {
    let isolation = isolation.ok_or(Error::ClaudeIsolationUnavailable)?;
    let paths = roots(ctx)?;
    let config = tmp_config_path(ctx.state_dir, ctx.turn_id);
    let id = ctx
        .thread
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| Error::Invalid)?
        .unwrap_or(new_id);
    let args = claude_args(&config, id, ctx.thread.is_some(), ctx.model);
    let (executable, args) =
        isolation.verified_command(&resolve_cli_binary(ctx.executable)?, &args, &paths)?;
    let mut env = environment();
    env.push((
        "CLAUDE_CONFIG_DIR".into(),
        home(ctx.state_dir, "claude")?.into_os_string(),
    ));
    let payload = json!({"mcpServers":{"host":{"type":"http","url":ctx.mcp_url,
        "headers":{"Authorization":format!("Bearer {}",ctx.token)}}}});
    write_private_file(
        &config,
        &serde_json::to_vec(&payload).map_err(|_| Error::Invalid)?,
    )?;
    Ok(WorkerCommand {
        executable,
        args,
        env,
        cwd: paths[0].clone(),
    })
}

pub fn parse_claude(line: &str) -> Option<AgentEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "system" if v["subtype"] == "init" => Some(AgentEvent::Session {
            id: v["session_id"].as_str()?.into(),
        }),
        "stream_event" if v["event"]["type"] == "content_block_delta" => {
            let d = &v["event"]["delta"];
            match d["type"].as_str()? {
                "text_delta" => Some(AgentEvent::TextDelta {
                    text: d["text"].as_str()?.into(),
                }),
                "thinking_delta" => Some(AgentEvent::ReasoningDelta {
                    text: d["thinking"].as_str()?.into(),
                }),
                _ => None,
            }
        }
        "assistant" => {
            let content = v["message"]["content"].as_array()?;
            let mut text = String::new();
            for block in content {
                if block["type"] == "text" {
                    if let Some(part) = block["text"].as_str() {
                        text.push_str(part);
                    }
                }
            }
            (!text.is_empty()).then_some(AgentEvent::TextDelta { text })
        }
        "result" => {
            let is_error = v["is_error"].as_bool().unwrap_or_else(|| {
                v["subtype"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("error"))
            });
            Some(AgentEvent::Finished { success: !is_error })
        }
        _ => None,
    }
}
