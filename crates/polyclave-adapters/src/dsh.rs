use crate::{
    base_environment, prepare_isolated_home, push_option, roots, Error, WorkerCommand,
    WorkerContext,
};
use polyclave_os::resolve_cli_binary;
use polyclave_proto::{AgentEvent, Vendor};
use serde_json::Value;
use std::{ffi::OsString, path::PathBuf};

pub fn dsh_args(roots: &[PathBuf], model: Option<&str>, thread: Option<&str>) -> Vec<OsString> {
    let mut args = [
        "--profile",
        "headless",
        "--json",
        "--permission-mode",
        "workspace-write",
        "--approval-policy",
        "never",
    ]
    .map(OsString::from)
    .to_vec();
    for root in roots {
        push_option(&mut args, "--add-dir", root);
    }
    if let Some(thread) = thread {
        push_option(&mut args, "--session-id", thread);
    }
    if let Some(model) = model {
        push_option(&mut args, "--model", model);
    }
    args.push("-".into());
    args
}

pub fn dsh_worker(ctx: &WorkerContext<'_>) -> Result<WorkerCommand, Error> {
    let paths = roots(ctx)?;
    let isolated_home = prepare_isolated_home(ctx.state_dir, Vendor::Dsh)?;
    let mut env = base_environment();
    env.push(("DSH_HOME".into(), isolated_home.into_os_string()));
    env.push(("POLYCLAVE_MCP_TOKEN".into(), ctx.token.into()));
    Ok(WorkerCommand {
        executable: resolve_cli_binary(ctx.executable)?,
        args: dsh_args(&paths, ctx.model, ctx.thread),
        env,
        cwd: paths[0].clone(),
    })
}

pub fn parse_dsh(line: &str) -> Option<AgentEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "session.started" | "init" => Some(AgentEvent::Session {
            id: v["session_id"].as_str()?.into(),
        }),
        "thread.started" => Some(AgentEvent::Session {
            id: v["thread_id"].as_str()?.into(),
        }),
        "item.completed" => {
            let item = &v["item"];
            let text = item["text"].as_str()?.to_owned();
            if text.is_empty() {
                return None;
            }
            match item["type"].as_str()? {
                "agent_message" | "message" | "assistant_message" => {
                    Some(AgentEvent::TextDelta { text })
                }
                "reasoning" | "thinking" => Some(AgentEvent::ReasoningDelta { text }),
                _ => None,
            }
        }
        "message.delta" | "text.delta" => {
            let text = v["delta"]
                .as_str()
                .or_else(|| v["text"].as_str())?
                .to_owned();
            (!text.is_empty()).then_some(AgentEvent::TextDelta { text })
        }
        "reasoning.delta" | "thinking.delta" => {
            let text = v["delta"]
                .as_str()
                .or_else(|| v["text"].as_str())?
                .to_owned();
            (!text.is_empty()).then_some(AgentEvent::ReasoningDelta { text })
        }
        "assistant" => {
            let text = v["content"]
                .as_str()
                .or_else(|| v["text"].as_str())?
                .to_owned();
            (!text.is_empty()).then_some(AgentEvent::TextDelta { text })
        }
        "turn.completed" | "run.completed" => Some(AgentEvent::Finished { success: true }),
        "turn.failed" | "run.failed" => Some(AgentEvent::Finished { success: false }),
        "result" => {
            let is_error = v["is_error"]
                .as_bool()
                .or_else(|| v["status"].as_str().map(|s| s != "success"))
                .unwrap_or(false);
            Some(AgentEvent::Finished { success: !is_error })
        }
        _ => None,
    }
}
