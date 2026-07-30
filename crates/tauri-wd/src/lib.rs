//! `tauri-wd` provides a W3C WebDriver CLI and native, test-only automation
//! integration for Tauri applications.
//!
//! The HTTP server is inert unless the app was launched with
//! `TAURI_AUTOMATION=true`. `tauri-wd` supplies the private port, bearer token,
//! and readiness channel automatically.

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
compile_error!("tauri-wd supports macOS, Windows, and Linux");

use std::{io, time::Duration};

use tauri::{
    Manager, Runtime,
    plugin::{Builder, TauriPlugin},
};

mod platform;
mod server;
mod webdriver;

pub mod capabilities;
pub mod config;
pub mod driver;
pub mod error;
pub mod launcher;

pub const AUTOMATION_ENV_VAR: &str = "TAURI_AUTOMATION";
pub const PORT_ENV_VAR: &str = "TAURI_WEBDRIVER_PORT";
pub const TOKEN_ENV_VAR: &str = "TAURI_WEBDRIVER_TOKEN";
pub const READY_FILE_ENV_VAR: &str = "TAURI_WEBDRIVER_READY_FILE";
pub const PROFILE_DIR_ENV_VAR: &str = "TAURI_AUTOMATION_PROFILE_DIR";
pub const STARTUP_TIMEOUT_ENV_VAR: &str = "TAURI_WEBDRIVER_STARTUP_TIMEOUT_MS";

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

fn startup_timeout() -> Duration {
    std::env::var(STARTUP_TIMEOUT_ENV_VAR)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .map(Duration::from_millis)
        .unwrap_or(Duration::from_secs(30))
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
    Builder::new("tauri-wd")
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
            tracing::info!("tauri-wd plugin initialized");
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

/// Runs the `tauri-wd` W3C WebDriver listener.
pub async fn serve(mut config: config::DriverConfig) -> Result<(), error::WebDriverError> {
    use std::{net::SocketAddr, sync::Arc};

    if !config.host.is_loopback() {
        return Err(error::WebDriverError::invalid_argument(
            "The WebDriver listener must use a loopback address",
        ));
    }
    let address = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|source| {
            error::WebDriverError::unknown(format!("Failed to bind {address}: {source}"))
        })?;
    let local_address = listener.local_addr().map_err(|source| {
        error::WebDriverError::unknown(format!("Failed to inspect listener: {source}"))
    })?;
    config.host = local_address.ip();
    config.port = local_address.port();
    let driver = Arc::new(driver::Driver::new(config)?);

    tracing::info!("tauri-wd listening on http://{local_address}");
    let shutdown_driver = driver.clone();
    let server = axum::serve(listener, driver.router()).with_graceful_shutdown(async move {
        shutdown_signal().await;
        shutdown_driver.shutdown().await;
    });
    server.await.map_err(|source| {
        error::WebDriverError::unknown(format!("WebDriver server failed: {source}"))
    })
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        let mut terminate = match signal(SignalKind::terminate()) {
            Ok(signal) => signal,
            Err(error) => {
                tracing::warn!("failed to register SIGTERM handler: {error}");
                return wait_for_ctrl_c().await;
            }
        };
        tokio::select! {
            () = wait_for_ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }

    #[cfg(not(unix))]
    wait_for_ctrl_c().await;
}

async fn wait_for_ctrl_c() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::warn!("failed to register shutdown signal: {error}");
        std::future::pending::<()>().await;
    }
}
