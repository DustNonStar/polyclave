//! Library entrypoint for Tauri v2 mobile targets (iOS / Android).
//! Desktop targets (`macos`, `windows`) boot through `main.rs` -> `shell::run()`.

#[cfg(any(target_os = "ios", target_os = "android"))]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .run(tauri::generate_context!())
        .expect("error while running Polyclave mobile application");
}
