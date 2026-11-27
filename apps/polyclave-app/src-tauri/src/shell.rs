use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    io::{self, Read},
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[cfg(target_os = "macos")]
use tauri::RunEvent;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, WindowEvent,
};
use tauri_plugin_notification::NotificationExt;
use zeroize::Zeroizing;

const ORIGIN: &str = "http://127.0.0.1:4310";

#[derive(Clone)]
pub struct Daemon {
    client: Client,
    state: Arc<Mutex<Option<Connection>>>,
}

impl Daemon {
    fn new() -> Self {
        let client = Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(2))
            .build()
            .expect("the loopback client could not start");
        Self {
            client,
            state: Arc::new(Mutex::new(None)),
        }
    }

    fn connect(&self, allow_spawn: bool) -> Result<Connection, String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "Daemon lock unavailable".to_owned())?;
        if let Some(conn) = guard.as_ref() {
            if check_connection(&self.client, conn) {
                return Ok(conn.clone());
            }
            *guard = None;
        }
        if let Some(conn) = probe(&self.client) {
            *guard = Some(conn.clone());
            return Ok(conn);
        }
        if !allow_spawn {
            return Err("The local daemon is not running".to_owned());
        }
        let mut child = if std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], 4310).into(),
            Duration::from_millis(150),
        )
        .is_err()
        {
            let companion = std::env::current_exe()
                .map_err(|e| format!("Desktop executable path unavailable: {e}"))?
                .with_file_name(if cfg!(windows) {
                    "polyclaved.exe"
                } else {
                    "polyclaved"
                });
            if !companion.is_file() {
                return Err(format!(
                    "The packaged polyclaved companion is missing at {}",
                    companion.display()
                ));
            }
            let mut command = Command::new(&companion);
            command
                .env("POLYCLAVE_PORT", "4310")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.creation_flags(0x08000000 | 0x00000200);
            }
            Some(command.spawn().map_err(|e| {
                format!(
                    "Could not start the local daemon ({}): {e}",
                    companion.display()
                )
            })?)
        } else {
            None
        };
        let due = Instant::now() + Duration::from_secs(6);
        loop {
            if let Some(conn) = probe(&self.client) {
                if let Some(mut c) = child.take() {
                    std::thread::spawn(move || {
                        let _ = c.wait();
                    });
                }
                *guard = Some(conn.clone());
                return Ok(conn);
            }
            if let Some(c) = child.as_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    let mut err = String::new();
                    if let Some(mut stderr) = c.stderr.take() {
                        let mut buf = Vec::new();
                        let _ = (&mut stderr).take(1024).read_to_end(&mut buf);
                        err = String::from_utf8_lossy(&buf).trim().to_owned();
                    }
                    return Err(if err.is_empty() {
                        format!("The local daemon exited early ({status})")
                    } else {
                        format!("The local daemon exited early ({status}): {err}")
                    });
                }
            }
            if Instant::now() >= due {
                if let Some(mut c) = child.take() {
                    std::thread::spawn(move || {
                        let _ = c.wait();
                    });
                }
                return Err(format!(
                    "The local daemon did not become ready at {ORIGIN} within 6s"
                ));
            }
            std::thread::park_timeout(Duration::from_millis(100));
        }
    }

    pub fn get(&self, path: &str) -> Result<Value, String> {
        self.get_with(path, true)
    }

    pub fn get_if_running(&self, path: &str) -> Result<Value, String> {
        self.get_with(path, false)
    }

    fn get_with(&self, path: &str, allow_spawn: bool) -> Result<Value, String> {
        let conn = self.connect(allow_spawn)?;
        let resp = self
            .client
            .get(format!("{ORIGIN}{path}"))
            .bearer_auth(&*conn.token)
            .send();
        match resp
            .and_then(|r| r.error_for_status())
            .and_then(|r| r.json())
        {
            Ok(v) => Ok(v),
            Err(e) => {
                if let Ok(mut guard) = self.state.lock() {
                    *guard = None;
                }
                Err(format!("The local daemon did not return {path}: {e}"))
            }
        }
    }
}

#[derive(Clone)]
pub struct Connection {
    token: Zeroizing<String>,
}

fn state_dir() -> Option<PathBuf> {
    std::env::var_os("POLYCLAVE_STATE_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|p| p.join("polyclave")))
}

fn check_connection(client: &Client, conn: &Connection) -> bool {
    client
        .get(format!("{ORIGIN}/api/sessions"))
        .bearer_auth(&*conn.token)
        .send()
        .is_ok_and(|r| r.status().is_success())
}

fn probe(client: &Client) -> Option<Connection> {
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
    check_connection(client, &connection).then_some(connection)
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
        let connection = daemon.connect(true)?;
        Ok(LocalConnection {
            origin: ORIGIN,
            bearer: connection.token.to_string(),
        })
    })
    .await
    .map_err(|_| "The loopback bootstrap was interrupted".to_owned())?
}

fn show_main_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn run() {
    let daemon = Daemon::new();
    let app = tauri::Builder::default()
        .manage(daemon.clone())
        .plugin(tauri_plugin_notification::init())
        .invoke_handler(tauri::generate_handler![
            local_connection,
            crate::keyring::key_status,
            crate::keyring::save_key,
            crate::keyring::delete_key
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
                    "open" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => (),
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { .. } = event {
                        show_main_window(tray.app_handle());
                    }
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
                        if let Ok(Value::Array(items)) = daemon.get_if_running(path) {
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
        .build(tauri::generate_context!())
        .expect("could not build the Polyclave desktop shell");

    app.run(|_app, _event| {
        #[cfg(target_os = "macos")]
        if let RunEvent::Reopen {
            has_visible_windows,
            ..
        } = _event
        {
            if !has_visible_windows {
                show_main_window(_app);
            }
        }
    });
}
