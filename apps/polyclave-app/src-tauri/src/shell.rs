use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    io,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;
use zeroize::Zeroizing;

const ORIGIN: &str = "http://127.0.0.1:4310";
#[derive(Clone)]
pub struct Daemon(Arc<Mutex<()>>);
impl Daemon {
    fn connection(&self) -> Result<Connection, &'static str> {
        let _guard = self.0.lock().map_err(|_| "Daemon lock unavailable")?;
        if let Some(connection) = probe() {
            return Ok(connection);
        }
        if std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], 4310).into(),
            Duration::from_millis(150),
        )
        .is_err()
        {
            let companion = std::env::current_exe()
                .map_err(|_| "Desktop executable path unavailable")?
                .with_file_name(if cfg!(windows) {
                    "polyclaved.exe"
                } else {
                    "polyclaved"
                });
            if !companion.is_file() {
                return Err("The packaged polyclaved companion is missing");
            }
            let mut command = Command::new(companion);
            command
                .env("POLYCLAVE_PORT", "4310")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000 | 0x00000200);
            }
            let mut child = command
                .spawn()
                .map_err(|_| "Could not start the local daemon")?;
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
        let due = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(connection) = probe() {
                return Ok(connection);
            }
            if Instant::now() >= due {
                return Err("The local daemon did not become ready");
            }
            std::thread::park_timeout(Duration::from_millis(100));
        }
    }
    pub fn get(&self, path: &str) -> Result<Value, &'static str> {
        let connection = self.connection()?;
        connection
            .client()?
            .get(format!("{ORIGIN}{path}"))
            .bearer_auth(&*connection.token)
            .send()
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
            .map_err(|_| "The local daemon did not return the requested data")
    }
}
pub struct Connection {
    token: Zeroizing<String>,
}
impl Connection {
    fn client(&self) -> Result<Client, &'static str> {
        Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .map_err(|_| "The loopback client could not start")
    }
}
fn state_dir() -> Option<PathBuf> {
    std::env::var_os("POLYCLAVE_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|p| p.join("polyclave")))
}
fn probe() -> Option<Connection> {
    let path = state_dir()?.join("local-auth.token");
    let metadata = path.symlink_metadata().ok()?;
    if !metadata.file_type().is_file() || metadata.len() > 128 {
        return None;
    }
    let token = std::fs::read_to_string(path).ok()?;
    if !matches!(token.len(), 32 | 64) || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let connection = Connection {
        token: Zeroizing::new(token),
    };
    let ok = connection
        .client()
        .ok()?
        .get(format!("{ORIGIN}/api/sessions"))
        .bearer_auth(&*connection.token)
        .send()
        .ok()?
        .status()
        .is_success();
    ok.then_some(connection)
}
#[derive(Serialize)]
struct LocalConnection {
    origin: &'static str,
    bearer: String,
}
#[tauri::command]
async fn local_connection(daemon: tauri::State<'_, Daemon>) -> Result<LocalConnection, String> {
    let daemon = daemon.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let connection = daemon.connection().map_err(str::to_owned)?;
        Ok(LocalConnection {
            origin: ORIGIN,
            bearer: connection.token.to_string(),
        })
    })
    .await
    .map_err(|_| "The loopback bootstrap was interrupted".to_owned())?
}
pub fn run() {
    let daemon = Daemon(Arc::new(Mutex::new(())));
    tauri::Builder::default()
        .manage(daemon.clone())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            local_connection,
            crate::keyring::key_status,
            crate::keyring::save_key
        ])
        .setup(move |app| {
            let open = MenuItem::with_id(app, "open", "Open Polyclave", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit desktop", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;
            let icon = app
                .default_window_icon()
                .ok_or_else(|| io::Error::other("desktop icon unavailable"))?
                .clone();
            TrayIconBuilder::new()
                .icon(icon)
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => (),
                })
                .build(app)?;
            let handle = app.handle().clone();
            let daemon = daemon.clone();
            std::thread::spawn(move || {
                let mut shown = HashSet::new();
                loop {
                    for (path, status, title) in [
                        ("/api/runs", "needs_review", "A Worker run needs review"),
                        (
                            "/api/tickets",
                            "blocked_on_user",
                            "A Ticket needs your attention",
                        ),
                    ] {
                        if let Ok(Value::Array(items)) = daemon.get(path) {
                            for item in items {
                                if let Some(id) = item["id"].as_str() {
                                    if item["status"] == status {
                                        if shown.insert((path, id.to_owned())) {
                                            let _ = handle
                                                .notification()
                                                .builder()
                                                .title("Polyclave")
                                                .body(title)
                                                .show();
                                        }
                                    } else {
                                        shown.remove(&(path, id.to_owned()));
                                    }
                                }
                            }
                        }
                    }
                    std::thread::park_timeout(Duration::from_secs(3));
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("could not start the Polyclave desktop shell");
}
