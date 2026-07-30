//! Cross-platform W3C WebDriver intermediary for Tauri applications.

pub mod capabilities;
pub mod config;
pub mod driver;
pub mod error;
pub mod launcher;

use std::{net::SocketAddr, sync::Arc};

use config::DriverConfig;
use driver::Driver;
use error::WebDriverError;

pub async fn serve(mut config: DriverConfig) -> Result<(), WebDriverError> {
    if !config.host.is_loopback() {
        return Err(WebDriverError::invalid_argument(
            "The WebDriver listener must use a loopback address",
        ));
    }
    let address = SocketAddr::new(config.host, config.port);
    let listener = tokio::net::TcpListener::bind(address)
        .await
        .map_err(|error| WebDriverError::unknown(format!("Failed to bind {address}: {error}")))?;
    let local_address = listener
        .local_addr()
        .map_err(|error| WebDriverError::unknown(format!("Failed to inspect listener: {error}")))?;
    config.host = local_address.ip();
    config.port = local_address.port();
    let driver = Arc::new(Driver::new(config)?);

    tracing::info!("tauri-wd listening on http://{local_address}");
    let shutdown_driver = driver.clone();
    let server = axum::serve(listener, driver.router()).with_graceful_shutdown(async move {
        shutdown_signal().await;
        shutdown_driver.shutdown().await;
    });
    server
        .await
        .map_err(|error| WebDriverError::unknown(format!("WebDriver server failed: {error}")))
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
