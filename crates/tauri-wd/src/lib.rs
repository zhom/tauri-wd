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
/// Set by the driver when a session asks for `tauri:options.headless`. The
/// plugin keeps the webview window off the user's screen (see `conceal_window`)
/// and, on macOS, runs the app as an accessory.
pub const HEADLESS_ENV_VAR: &str = "TAURI_WEBDRIVER_HEADLESS";

/// Conceal a window for a headless session.
///
/// On macOS a truly hidden (`orderOut:`) window makes WebKit treat the page as
/// non-visible and suspends its rendering, timers, and `requestAnimationFrame`
/// loop, which stalls every animation- or rAF-gated WebDriver command until the
/// script timeout. Instead, keep the window ordered-in but fully transparent,
/// click-through, and floated above other windows on every Space, so nothing is
/// ever shown, input passes through, and -- because nothing ever covers it --
/// the webview keeps rendering. Focus is never taken: the window is made
/// non-focusable, `orderFrontRegardless` does not make it key, and the app runs
/// as an accessory. On Windows and Linux the window is simply hidden, which
/// pauses `requestAnimationFrame` there; see the README.
///
/// A macro, not a function, because `on_webview_ready` yields a `Window` while
/// `webview_windows()` yields a `WebviewWindow`; both expose these inherent
/// methods but share no public trait.
macro_rules! conceal_window {
    ($window:expr) => {{
        let window = $window;
        #[cfg(target_os = "macos")]
        {
            match window.ns_window() {
                Ok(ptr) if !ptr.is_null() => {
                    let ns_window = ptr as *mut objc2::runtime::AnyObject;
                    // Safety: `ns_window()` returns this window's live `NSWindow`
                    // and the plugin hooks run on the main thread.
                    //
                    // Conceal the window without hiding it. macOS suspends a
                    // WKWebView's rendering, timers, and requestAnimationFrame
                    // loop whenever its window is off-screen or fully occluded,
                    // so a hidden (`orderOut:`) or off-screen window stalls every
                    // animation- or rAF-gated command until the script timeout.
                    // Instead: make it fully transparent and click-through (never
                    // seen, never intercepts input), float it above normal
                    // windows and onto every Space so nothing ever covers it (it
                    // stays unoccluded, so WebKit keeps rendering), and order it
                    // in with `orderFrontRegardless`, which does NOT make it key,
                    // so the accessory app never steals focus. These are all
                    // public AppKit selectors and work across macOS versions;
                    // the old private occlusion-detection switch is gone on
                    // macOS 26 and sending it aborts the process.
                    const NS_FLOATING_WINDOW_LEVEL: isize = 3;
                    const NS_WINDOW_CAN_JOIN_ALL_SPACES: usize = 1;
                    // A window that can never become key cannot take focus,
                    // whatever a later `show()` or `makeKeyAndOrderFront` does.
                    if let Err(error) = window.set_focusable(false) {
                        tracing::warn!("headless: could not make the window non-focusable: {error}");
                    }
                    unsafe {
                        let _: () = objc2::msg_send![ns_window, setAlphaValue: 0.0f64];
                        let _: () = objc2::msg_send![
                            ns_window,
                            setIgnoresMouseEvents: objc2::runtime::Bool::YES
                        ];
                        let _: () = objc2::msg_send![ns_window, setLevel: NS_FLOATING_WINDOW_LEVEL];
                        // OR into the existing behaviour rather than replace it,
                        // the way tao's own helper does.
                        let behaviour: usize = objc2::msg_send![ns_window, collectionBehavior];
                        let _: () = objc2::msg_send![
                            ns_window,
                            setCollectionBehavior: behaviour | NS_WINDOW_CAN_JOIN_ALL_SPACES
                        ];
                        let _: () = objc2::msg_send![ns_window, orderFrontRegardless];
                    }
                }
                Ok(_) => {
                    tracing::warn!("headless: ns_window was null; hiding the window instead");
                    let _ = window.hide();
                }
                Err(error) => {
                    tracing::warn!("headless: ns_window unavailable ({error}); hiding instead");
                    let _ = window.hide();
                }
            }
        }
        #[cfg(not(target_os = "macos"))]
        if let Err(error) = window.hide() {
            tracing::warn!("headless: could not hide the window: {error}");
        }
    }};
}

/// Returns whether this process was launched specifically for automation.
#[must_use]
pub fn automation_enabled() -> bool {
    std::env::var(AUTOMATION_ENV_VAR)
        .is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

/// Returns whether this automation process was asked to run headless.
///
/// The plugin conceals every webview window once it is ready, but it cannot
/// undo what creating the window already did: a window built visible and
/// focused activates the app and takes key focus for the moment before the
/// plugin runs. An app that wants a headless session to change NOTHING on the
/// user's screen reads this and creates its main window with `.visible(false)`
/// (or `.focused(false)`), and on macOS sets `ActivationPolicy::Accessory`
/// through `App::set_activation_policy` in its own setup, so no Dock tile ever
/// appears. Every WebDriver command works either way.
#[must_use]
pub fn headless_enabled() -> bool {
    automation_enabled()
        && std::env::var(HEADLESS_ENV_VAR)
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
            app.manage(platform::ScriptExecutionLocks::default());
            app.manage(platform::AlertStateManager::default());

            server::start(
                app.app_handle().clone(),
                port,
                token,
                std::path::PathBuf::from(ready_file),
            );

            if headless_enabled() {
                // macOS: an accessory app has no Dock tile and never becomes
                // the active application. Set here AND re-applied on
                // `RunEvent::Ready` below: tao writes its own (regular) policy
                // back at launch, undoing a value set through the handle before
                // the event loop starts. A Dock tile that never appears at all
                // needs the host to set the policy through
                // `App::set_activation_policy` in its own setup (see README).
                #[cfg(target_os = "macos")]
                if let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
                    tracing::warn!(
                        "headless: could not set the accessory activation policy: {error}"
                    );
                }
                // Only a window an earlier plugin's setup created can exist yet;
                // the app's config and setup windows arrive after launch and are
                // caught in `on_webview_ready`.
                for (_label, window) in app.webview_windows() {
                    conceal_window!(window);
                }
            }

            tracing::info!("tauri-wd plugin initialized");
            Ok(())
        })
        .on_event(|app, event| {
            #[cfg(target_os = "macos")]
            {
                if !matches!(event, tauri::RunEvent::Ready) || !headless_enabled() {
                    return;
                }
                if let Err(error) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
                    tracing::warn!(
                        "headless: could not re-apply the accessory activation policy: {error}"
                    );
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        })
        .on_webview_ready(|webview| {
            if !automation_enabled() {
                return;
            }
            platform::register_webview_handlers(&webview);
            if headless_enabled() {
                // The window backing this webview is being shown; conceal it so
                // the suite runs without a window ever appearing. On macOS it
                // stays on screen but transparent, click-through and never key,
                // so the webview keeps rendering (see `conceal_window`);
                // elsewhere it is hidden.
                conceal_window!(webview.window());
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
