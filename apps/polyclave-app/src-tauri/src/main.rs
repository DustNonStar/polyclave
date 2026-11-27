#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

#[cfg(any(target_os = "macos", windows))]
mod keyring;
#[cfg(any(target_os = "macos", windows))]
mod shell;

#[cfg(any(target_os = "macos", windows))]
fn main() {
    shell::run();
}
#[cfg(not(any(target_os = "macos", windows)))]
fn main() {
    eprintln!("The Phase 1B desktop shell is available on macOS and Windows.");
}
