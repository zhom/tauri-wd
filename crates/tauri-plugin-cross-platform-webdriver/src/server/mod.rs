use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

use tauri::{AppHandle, Manager, Runtime};
use tokio::{runtime::Runtime as TokioRuntime, sync::RwLock};

pub mod handlers;
pub mod response;
pub mod router;

use crate::platform::{FrameId, PlatformExecutor, create_executor};
use crate::server::response::WebDriverErrorResponse;
use crate::webdriver::{SessionManager, Timeouts};

pub struct AppState<R: Runtime> {
    pub app: AppHandle<R>,
    pub sessions: RwLock<SessionManager>,
}

impl<R: Runtime + 'static> AppState<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self {
            app,
            sessions: RwLock::new(SessionManager::new()),
        }
    }

    pub fn get_executor_for_window(
        &self,
        window_label: &str,
        timeouts: Timeouts,
        frame_context: Vec<FrameId>,
    ) -> Result<Arc<dyn PlatformExecutor<R>>, WebDriverErrorResponse> {
        self.app
            .webview_windows()
            .get(window_label)
            .cloned()
            .map(|window| create_executor(window, timeouts, frame_context))
            .ok_or_else(WebDriverErrorResponse::no_such_window)
    }

    pub fn get_window_labels(&self) -> Vec<String> {
        self.app.webview_windows().keys().cloned().collect()
    }
}

/// Starts a token-protected loopback server and publishes its actual port only
/// after the listener is ready.
pub fn start<R: Runtime + 'static>(
    app: AppHandle<R>,
    port: u16,
    token: String,
    ready_file: PathBuf,
) {
    std::thread::spawn(move || {
        let rt = match TokioRuntime::new() {
            Ok(rt) => rt,
            Err(error) => {
                publish_error(&ready_file, &error.to_string());
                tracing::error!("Failed to create WebDriver runtime: {error}");
                return;
            }
        };

        rt.block_on(async {
            let address = SocketAddr::from(([127, 0, 0, 1], port));
            let listener = match tokio::net::TcpListener::bind(address).await {
                Ok(listener) => listener,
                Err(error) => {
                    publish_error(&ready_file, &error.to_string());
                    tracing::error!("Failed to bind private WebDriver server: {error}");
                    return;
                }
            };
            let actual = match listener.local_addr() {
                Ok(address) => address,
                Err(error) => {
                    publish_error(&ready_file, &error.to_string());
                    tracing::error!("Failed to inspect private WebDriver listener: {error}");
                    return;
                }
            };
            let state = Arc::new(AppState::new(app));
            let router = router::create_router(state, token);
            if let Err(error) = publish_ready(&ready_file, actual.port()) {
                tracing::error!("Failed to publish WebDriver readiness: {error}");
                return;
            }

            tracing::info!("Private WebDriver server listening on {actual}");
            if let Err(error) = axum::serve(listener, router).await {
                tracing::error!("WebDriver server error: {error}");
            }
        });
    });
}

fn publish_ready(path: &Path, port: u16) -> std::io::Result<()> {
    publish(path, &serde_json::json!({ "port": port }))
}

fn publish_error(path: &Path, message: &str) {
    let _ = publish(path, &serde_json::json!({ "error": message }));
}

fn publish(path: &Path, value: &serde_json::Value) -> std::io::Result<()> {
    let temporary = path.with_extension("tmp");
    std::fs::write(&temporary, value.to_string())?;
    std::fs::rename(temporary, path)
}
