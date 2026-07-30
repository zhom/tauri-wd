use std::sync::Arc;

use axum::extract::State;
use serde_json::json;
use tauri::Runtime;

use super::AppState;
use super::response::{WebDriverResponse, WebDriverResult};

pub mod actions;
pub mod alert;
pub mod cookie;
pub mod document;
pub mod element;
pub mod frame;
pub mod navigation;
pub mod print;
pub mod screenshot;
pub mod script;
pub mod session;
pub mod shadow;
pub mod timeouts;
pub mod window;

/// Returns ready=true only when at least one webview window is available.
/// This ensures the WebView2/WebKit/WKWebView is fully initialized before
/// clients attempt to create sessions.
pub async fn status<R: Runtime>(state: State<Arc<AppState<R>>>) -> WebDriverResult {
    let window_labels = state.get_window_labels();
    let ready = !window_labels.is_empty();

    Ok(WebDriverResponse::success(json!({
        "ready": ready,
        "message": if ready {
            "tauri-wd is ready"
        } else {
            "waiting for webview initialization"
        }
    })))
}
