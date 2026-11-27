use crate::{environment, home, push_option, roots, Error, WorkerCommand, WorkerContext};
use polyclave_os::resolve_cli_binary;
use polyclave_proto::AgentEvent;
use serde_json::Value;
use std::{ffi::OsString, path::PathBuf};

pub fn codex_args(
    roots: &[PathBuf],
    url: &str,
    model: Option<&str>,
    thread: Option<&str>,
) -> Result<Vec<OsString>, Error> {
    let mut args = vec!["exec".into()];
    if thread.is_some() {
        args.push("resume".into());
    }
    args.extend(["--json", "--skip-git-repo-check"].map(OsString::from));
    for conf in [
        "approval_policy=\"never\"",
        "sandbox_mode=\"workspace-write\"",
        "sandbox_workspace_write.network_access=false",
        "mcp_servers.host.bearer_token_env_var=\"POLYCLAVE_MCP_TOKEN\"",
        "mcp_servers.host.default_tools_approval_mode=\"approve\"",
    ] {
        push_option(&mut args, "-c", conf);
    }
    push_option(
        &mut args,
        "-c",
        format!(
            "mcp_servers.host.url={}",
            serde_json::to_string(url).map_err(|_| Error::Invalid)?
        ),
    );
    let roots: Option<Vec<_>> = roots.iter().map(|r| r.to_str()).collect();
    push_option(
        &mut args,
        "-c",
        format!(
            "sandbox_workspace_write.writable_roots={}",
            serde_json::to_string(&roots.ok_or(Error::Invalid)?).map_err(|_| Error::Invalid)?
        ),
    );
    if let Some(model) = model {
        push_option(&mut args, "--model", model);
    }
    args.push("--".into());
    if let Some(thread) = thread {
        args.push(thread.into());
    }
    args.push("-".into());
    Ok(args)
}

pub fn codex_worker(ctx: &WorkerContext<'_>) -> Result<WorkerCommand, Error> {
    let paths = roots(ctx)?;
    let mut env = environment();
    env.push((
        "CODEX_HOME".into(),
        home(ctx.state_dir, "codex")?.into_os_string(),
    ));
    env.push(("POLYCLAVE_MCP_TOKEN".into(), ctx.token.into()));
    Ok(WorkerCommand {
        executable: resolve_cli_binary(ctx.executable)?,
        args: codex_args(&paths, ctx.mcp_url, ctx.model, ctx.thread)?,
        env,
        cwd: paths[0].clone(),
    })
}

pub fn parse_codex(line: &str) -> Option<AgentEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "thread.started" => Some(AgentEvent::Session {
            id: v["thread_id"].as_str()?.into(),
        }),
        "item.completed" => {
            let item = &v["item"];
            let text = item["text"].as_str()?.to_owned();
            match item["type"].as_str()? {
                "agent_message" => Some(AgentEvent::TextDelta { text }),
                "reasoning" => Some(AgentEvent::ReasoningDelta { text }),
                _ => None,
            }
        }
        "turn.completed" => Some(AgentEvent::Finished { success: true }),
        "turn.failed" => Some(AgentEvent::Finished { success: false }),
        _ => None,
    }
}
