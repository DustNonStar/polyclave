//! Offline Worker executable; it is never selected by the daemon.
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    io::{Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpStream},
    process::Command,
};

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args
        .first()
        .is_some_and(|s| polyclave_os::is_supervisor_argument(s))
    {
        std::process::exit(polyclave_os::supervisor_exec_entry(&args[1..]).unwrap_or(125));
    }
    std::process::exit(worker(&args));
}

fn worker(args: &[OsString]) -> i32 {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    if std::env::vars_os().any(|(k, _)| {
        let k = k.to_string_lossy().to_ascii_uppercase();
        k == "GH_TOKEN"
            || k == "GITHUB_TOKEN"
            || (k.starts_with("POLYCLAVE_") && k != "POLYCLAVE_MCP_TOKEN")
    }) {
        return 30;
    }
    let key = std::env::var("POLYCLAVE_MCP_TOKEN").unwrap();
    println!(
        "{}",
        json!({"type":"thread.started","thread_id":"fixture-thread"})
    );
    // A CLI accidentally printing its own turn credential must not reach SSE.
    println!(
        "{}",
        json!({"type":"item.completed","item":{"type":"agent_message","text":key}})
    );
    let n = std::fs::read_to_string(".fixture-turn")
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(0)
        + 1;
    std::fs::write(".fixture-turn", n.to_string()).unwrap();
    if let Some(address) = field(&input, "Fixture barrier: ") {
        let address: SocketAddr = address.parse().unwrap();
        assert!(address.ip().is_loopback());
        let mut s = TcpStream::connect(address).unwrap();
        s.write_all(b"started\n").unwrap();
        let _ = s.read_exact(&mut [0]);
    }
    let run = field(&input, "Run: ").unwrap();
    let branch = field(&input, "Attempt branch: ").unwrap();
    if input.contains("Fixture: no result") {
        std::fs::write(".fixture-no-result", b"yes").unwrap();
    }
    if std::path::Path::new(".fixture-no-result").exists()
        || (n == 1 && input.contains("Fixture: first turn asks for correction"))
    {
        return 0;
    }
    let sha = commit(run, n);
    push(branch);
    if input.contains("Fixture: no receipt") {
        return 0;
    }
    let raw = args
        .iter()
        .filter_map(|s| s.to_str())
        .find_map(|s| s.strip_prefix("mcp_servers.host.url="))
        .unwrap();
    let url: String = serde_json::from_str(raw).unwrap();
    let response = rpc(&url, &key, json!({"run_id":run,"branch":branch,"sha":sha}));
    assert!(matches!(
        response["result"]["isError"].as_bool(),
        Some(false)
    ));
    if input.contains("Fixture: move tip after receipt") {
        commit(run, n + 1);
        push(branch);
    }
    if input.contains("Fixture: stdout burst before exit") {
        for _ in 0..280 {
            println!(
                "{}",
                json!({"type":"item.completed","item":{"type":"agent_message","text":"burst"}})
            );
        }
    }
    if input.contains("Fixture: nonzero after receipt") {
        19
    } else {
        0
    }
}

fn field<'a>(text: &'a str, label: &str) -> Option<&'a str> {
    text.lines().find_map(|s| s.strip_prefix(label))
}

fn git(args: &[&str]) -> String {
    let result = Command::new("git").args(args).output().unwrap();
    if !result.status.success() {
        std::process::exit(31);
    }
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}
fn commit(run: &str, n: u32) -> String {
    let file = format!("result-{n}.txt");
    std::fs::write(&file, n.to_string()).unwrap();
    git(&["add", &file]);
    git(&[
        "-c",
        "user.name=Fixture",
        "-c",
        "user.email=fixture@example.invalid",
        "-c",
        "commit.gpgSign=false",
        "commit",
        "-m",
        "fixture Worker change",
        "--trailer",
        &format!("Run-Id: {run}"),
    ]);
    git(&["rev-parse", "HEAD"])
}
fn push(branch: &str) {
    let r = format!("refs/heads/{branch}");
    git(&["push", "polyclave-staging", &format!("{r}:{r}")]);
}
fn rpc(url: &str, key: &str, data: Value) -> Value {
    let (address, path) = url
        .strip_prefix("http://")
        .unwrap()
        .split_once('/')
        .unwrap();
    let addr: SocketAddr = address.parse().unwrap();
    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
    let mut s = TcpStream::connect(addr).unwrap();
    let body=json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"submit_result","arguments":data}}).to_string();
    write!(s,"POST /{path} HTTP/1.1\r\nHost: {address}\r\nAuthorization: Bearer {key}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
    let mut response = String::new();
    s.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("HTTP/1.1 200"));
    serde_json::from_str(body).unwrap()
}
