use crate::{
    base_environment, prepare_isolated_home, push_option, roots, tmp_config_path, Error,
    WorkerCommand, WorkerContext,
};
use polyclave_os::{resolve_cli_binary, write_private_file};
use polyclave_proto::{AgentEvent, Vendor};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};

pub fn agy_args(
    roots: &[PathBuf],
    config: &Path,
    model: Option<&str>,
    thread: Option<&str>,
) -> Vec<OsString> {
    let mut args = [
        "-p",
        "--output-format",
        "stream-json",
        "--sandbox",
        "--mode",
        "auto",
    ]
    .map(OsString::from)
    .to_vec();
    for root in roots {
        push_option(&mut args, "--add-dir", root);
    }
    push_option(&mut args, "--settings", config);
    if let Some(thread) = thread {
        push_option(&mut args, "--resume", thread);
    }
    if let Some(model) = model {
        push_option(&mut args, "--model", model);
    }
    args
}

pub fn agy_worker(ctx: &WorkerContext<'_>) -> Result<WorkerCommand, Error> {
    let paths = roots(ctx)?;
    let config = tmp_config_path(ctx.state_dir, ctx.turn_id);
    let isolated_home = prepare_isolated_home(ctx.state_dir, Vendor::Agy)?;
    let payload = json!({
        "mcpServers": {
            "host": {
                "type": "http",
                "url": ctx.mcp_url,
                "headers": {
                    "Authorization": format!("Bearer {}", ctx.token)
                }
            }
        }
    });
    write_private_file(
        &config,
        &serde_json::to_vec(&payload).map_err(|_| Error::Invalid)?,
    )?;
    let mut env = base_environment();
    env.push(("GEMINI_HOME".into(), isolated_home.clone().into_os_string()));
    env.push(("AGY_HOME".into(), isolated_home.into_os_string()));
    env.push(("POLYCLAVE_MCP_TOKEN".into(), ctx.token.into()));
    Ok(WorkerCommand {
        executable: resolve_cli_binary(ctx.executable)?,
        args: agy_args(&paths, &config, ctx.model, ctx.thread),
        env,
        cwd: paths[0].clone(),
    })
}

pub fn parse_agy(line: &str) -> Option<AgentEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "init" => Some(AgentEvent::Session {
            id: v["session_id"].as_str()?.into(),
        }),
        "system" if v["subtype"] == "init" => Some(AgentEvent::Session {
            id: v["session_id"].as_str()?.into(),
        }),
        "message" => {
            let role = v["role"].as_str();
            let text = v["content"]
                .as_str()
                .or_else(|| v["text"].as_str())?
                .to_owned();
            if text.is_empty() {
                return None;
            }
            match role {
                Some("assistant") | Some("model") | None => Some(AgentEvent::TextDelta { text }),
                Some("thought") | Some("thinking") => Some(AgentEvent::ReasoningDelta { text }),
                _ => None,
            }
        }
        "thought" | "thinking" => {
            let text = v["content"]
                .as_str()
                .or_else(|| v["text"].as_str())?
                .to_owned();
            (!text.is_empty()).then_some(AgentEvent::ReasoningDelta { text })
        }
        "content" | "text" => {
            let text = v["text"]
                .as_str()
                .or_else(|| v["content"].as_str())?
                .to_owned();
            (!text.is_empty()).then_some(AgentEvent::TextDelta { text })
        }
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
        "result" => {
            let is_error = v["is_error"]
                .as_bool()
                .or_else(|| v["status"].as_str().map(|s| s != "success"))
                .unwrap_or_else(|| {
                    v["subtype"]
                        .as_str()
                        .is_some_and(|s| s.starts_with("error"))
                });
            Some(AgentEvent::Finished { success: !is_error })
        }
        _ => None,
    }
}
