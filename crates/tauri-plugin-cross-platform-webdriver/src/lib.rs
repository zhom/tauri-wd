//! Native, test-only WebDriver integration for Tauri applications.
//!
//! The HTTP server is inert unless the app was launched with
//! `TAURI_AUTOMATION=true`. `tauri-wd` supplies the private port, bearer token,
//! and readiness channel automatically.

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
compile_error!("tauri-plugin-cross-platform-webdriver supports macOS, Windows, and Linux");

use std::io;

use tauri::{
    Manager, Runtime,
    plugin::{Builder, TauriPlugin},
};

mod platform;
mod server;
mod webdriver;

pub const AUTOMATION_ENV_VAR: &str = "TAURI_AUTOMATION";
pub const PORT_ENV_VAR: &str = "TAURI_WEBDRIVER_PORT";
pub const TOKEN_ENV_VAR: &str = "TAURI_WEBDRIVER_TOKEN";
pub const READY_FILE_ENV_VAR: &str = "TAURI_WEBDRIVER_READY_FILE";
pub const PROFILE_DIR_ENV_VAR: &str = "TAURI_AUTOMATION_PROFILE_DIR";

/// Returns whether this process was launched specifically for automation.
#[must_use]
pub fn automation_enabled() -> bool {
    std::env::var(AUTOMATION_ENV_VAR)
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Returns the isolated per-session directory created by `tauri-wd`.
///
/// Applications can store test fixtures, downloads, and profile-scoped data
/// here to keep concurrent sessions deterministic.
#[must_use]
pub fn automation_profile_dir() -> Option<std::path::PathBuf> {
    automation_enabled()
        .then(|| std::env::var_os(PROFILE_DIR_ENV_VAR).map(std::path::PathBuf::from))
        .flatten()
}

/// Initializes the plugin. The server only starts in an automation process.
#[must_use]
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    let port = std::env::var(PORT_ENV_VAR)
        .ok()
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    init_with_port(port)
}

/// Initializes the plugin with an explicit local port.
///
/// Port `0` asks the operating system for an unused port and is recommended.
#[must_use]
pub fn init_with_port<R: Runtime>(port: u16) -> TauriPlugin<R> {
    Builder::new("cross-platform-webdriver")
        .setup(move |app, _api| {
            if !automation_enabled() {
                tracing::debug!("WebDriver plugin is inert outside automation builds");
                return Ok(());
            }

            let token = required_env(TOKEN_ENV_VAR)?;
            let ready_file = required_env(READY_FILE_ENV_VAR)?;

            #[cfg(target_os = "windows")]
            app.manage(platform::AsyncScriptState::default());
            #[cfg(target_os = "windows")]
            app.manage(platform::ScriptExecutionLocks::default());
            app.manage(platform::AlertStateManager::default());

            server::start(
                app.app_handle().clone(),
                port,
                token,
                std::path::PathBuf::from(ready_file),
            );
            tracing::info!("cross-platform WebDriver plugin initialized");
            Ok(())
        })
        .on_webview_ready(|webview| {
            if automation_enabled() {
                platform::register_webview_handlers(&webview);
            }
        })
        .build()
}

fn required_env(name: &str) -> std::result::Result<String, io::Error> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} is required when automation is enabled"),
            )
        })
}
