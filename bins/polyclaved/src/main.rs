mod auth;
mod routes;

use polyclave_adapters::mcp::Mcp;
use polyclave_core::{
    mirror::MirrorManager,
    runner::{RunnerConfig, TurnRunner},
    runs::RunManager,
};
use polyclave_os::{
    create_private_dir, is_supervisor_argument, resolve_cli_binary, supervisor_exec_entry,
    write_private_file,
};
use polyclave_store::Store;
use std::{
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    sync::Arc,
};
use uuid::Uuid;

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|s| is_supervisor_argument(s)) {
        std::process::exit(supervisor_exec_entry(&args[1..]).unwrap_or(125));
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build();
    if runtime.and_then(|rt| rt.block_on(serve())).is_err() {
        eprintln!("Polyclave could not start or serve the local daemon. Check the state directory, daemon lock, and loopback port.");
        std::process::exit(1);
    }
}

async fn serve() -> io::Result<()> {
    let state = state_dir()?;
    let _lock = lock_host(&state)?;
    let port = std::env::var("POLYCLAVE_PORT")
        .ok()
        .map(|s| s.parse::<u16>())
        .transpose()
        .map_err(|_| io::Error::other("invalid loopback port"))?
        .unwrap_or(4310);
    let api = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let port = api.local_addr()?.port();
    let store = Store::open(state.join("polyclave.sqlite")).map_err(io::Error::other)?;
    let runs = RunManager::new(
        store.clone(),
        Arc::new(MirrorManager::new(&state).map_err(io::Error::other)?),
    );
    let host = Uuid::new_v4().simple().to_string();
    let (endpoint, mcp) = Mcp::new(store.clone(), host.clone(), Arc::new(runs.clone()))
        .listen()
        .await?;
    let (stream, _) = tokio::sync::broadcast::channel(256);
    let stream_writer = stream.clone();
    let runner = TurnRunner::new(
        runs.clone(),
        &state,
        host,
        RunnerConfig {
            supervisor: std::env::current_exe()?,
            mcp_url: format!("http://{endpoint}/mcp"),
            codex: resolve_cli_binary(Path::new("codex"))
                .unwrap_or_else(|_| PathBuf::from("codex")),
            claude: PathBuf::from("claude"),
            claude_isolation: None,
        },
        move |run, ev| {
            let _ = stream_writer.send((run.to_owned(), ev));
        },
    )
    .map_err(io::Error::other)?;
    runner.recover().map_err(io::Error::other)?;
    let token = Uuid::new_v4().simple().to_string();
    let token_file = state.join("local-auth.token");
    remove_if_present(&token_file)?;
    write_private_file(&token_file, token.as_bytes())?;
    let app = routes::router(
        routes::App {
            store,
            runs,
            runner,
            stream,
        },
        auth::LocalAuth::new(token.clone(), port),
    );
    println!("http://127.0.0.1:{port}/#token={token}");
    let result = axum::serve(api, app)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    mcp.abort();
    let _ = remove_if_present(&token_file);
    result
}

fn state_dir() -> io::Result<PathBuf> {
    let p = if let Some(p) = std::env::var_os("POLYCLAVE_STATE_DIR") {
        PathBuf::from(p)
    } else {
        dirs::data_local_dir()
            .ok_or_else(|| io::Error::other("no local data directory"))?
            .join("polyclave")
    };
    if let Some(parent) = p.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    match create_private_dir(&p) {
        Ok(()) => (),
        Err(e)
            if e.kind() == io::ErrorKind::AlreadyExists
                && p.symlink_metadata()?.file_type().is_dir() => {}
        Err(e) => return Err(e),
    }
    polyclave_os::canonical_real_path(&p)
}

fn lock_host(state: &Path) -> io::Result<std::fs::File> {
    let lock = state.join("host.lock");
    match write_private_file(&lock, b"") {
        Ok(()) => (),
        Err(e)
            if e.kind() == io::ErrorKind::AlreadyExists
                && lock.symlink_metadata()?.file_type().is_file() => {}
        Err(e) => return Err(e),
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock)?;
    file.try_lock().map_err(io::Error::other)?;
    Ok(file)
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
