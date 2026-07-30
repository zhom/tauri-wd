use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tauri::Runtime;

use crate::server::AppState;
use crate::server::handlers::timeouts::parse_timeout_value;
use crate::server::response::{WebDriverErrorResponse, WebDriverResponse, WebDriverResult};
use crate::startup_timeout;
use crate::webdriver::{PageLoadStrategy, Timeouts};

async fn wait_for_window<R: Runtime>(
    state: &AppState<R>,
    timeout: std::time::Duration,
) -> Result<String, WebDriverErrorResponse> {
    let start = std::time::Instant::now();
    let poll_interval = std::time::Duration::from_millis(100);

    loop {
        let window_labels = state.get_window_labels();

        if let Some(label) = window_labels.first().cloned() {
            return Ok(label);
        }

        if start.elapsed() >= timeout {
            return Err(WebDriverErrorResponse::no_such_window());
        }

        tokio::time::sleep(poll_interval).await;
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub capabilities: Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionResponse {
    pub session_id: String,
    pub capabilities: Value,
}

fn parse_user_agent(user_agent: &str) -> (String, String) {
    // Windows WebView2: "... Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0"
    if user_agent.contains("Edg/") {
        let version = user_agent
            .split("Edg/")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("unknown");
        return ("msedge".to_string(), version.to_string());
    }

    // Linux WebKitGTK: "... (X11; Linux ...) AppleWebKit/... Version/2.44..."
    if user_agent.contains("Linux") || user_agent.contains("X11") {
        let version = user_agent
            .split("AppleWebKit/")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .unwrap_or("unknown");
        return ("WebKitGTK".to_string(), version.to_string());
    }

    // macOS WebKit/WKWebView: "... (Macintosh; ...) AppleWebKit/605.1.15 ..."
    // Note: WKWebView may not include "Safari/" or "Version/"
    if user_agent.contains("Macintosh") && user_agent.contains("AppleWebKit/") {
        let version = user_agent
            .split("AppleWebKit/")
            .nth(1)
            .and_then(|s| s.split_whitespace().next())
            .and_then(|s| s.split('(').next()) // Remove trailing (KHTML if present
            .unwrap_or("unknown");
        return ("webkit".to_string(), version.to_string());
    }

    ("webview".to_string(), "unknown".to_string())
}

pub async fn create<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Json(request): Json<CreateSessionRequest>,
) -> WebDriverResult {
    let always_match = request
        .capabilities
        .get("alwaysMatch")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let page_load_strategy = match always_match
        .get("pageLoadStrategy")
        .and_then(Value::as_str)
        .unwrap_or("normal")
    {
        "none" => PageLoadStrategy::None,
        "eager" => PageLoadStrategy::Eager,
        "normal" => PageLoadStrategy::Normal,
        value => {
            return Err(WebDriverErrorResponse::invalid_argument(&format!(
                "Unsupported pageLoadStrategy: {value}"
            )));
        }
    };
    let mut requested_timeouts = Timeouts::default();
    if let Some(timeouts) = always_match.get("timeouts") {
        let timeouts = timeouts.as_object().ok_or_else(|| {
            WebDriverErrorResponse::invalid_argument("timeouts capability must be an object")
        })?;
        if let Some(value) = timeouts.get("implicit") {
            requested_timeouts.implicit_ms = parse_timeout_value(value, "implicit", true)?;
        }
        if let Some(value) = timeouts.get("pageLoad") {
            requested_timeouts.page_load_ms = parse_timeout_value(value, "pageLoad", true)?;
        }
        if let Some(value) = timeouts.get("script") {
            requested_timeouts.script_ms = parse_timeout_value(value, "script", true)?;
        }
    }
    let initial_window = wait_for_window(&state, startup_timeout()).await?;

    let executor =
        state.get_executor_for_window(&initial_window, Timeouts::default(), Vec::new())?;
    let user_agent_result = executor
        .evaluate_js("(function() { return navigator.userAgent; })()")
        .await;

    let (browser_name, browser_version) = match user_agent_result {
        Ok(result) => {
            let user_agent = result.get("value").and_then(|v| v.as_str()).unwrap_or("");
            parse_user_agent(user_agent)
        }
        Err(_) => ("webview".to_string(), "unknown".to_string()),
    };

    let mut sessions = state.sessions.write().await;

    let session = sessions.create(initial_window, page_load_strategy, requested_timeouts);

    let response = SessionResponse {
        session_id: session.id.clone(),
        capabilities: json!({
            "browserName": browser_name,
            "browserVersion": browser_version,
            "platformName": std::env::consts::OS,
            "acceptInsecureCerts": false,
            "pageLoadStrategy": session.page_load_strategy.as_str(),
            "setWindowRect": true,
            "timeouts": {
                "implicit": session.timeouts.implicit_ms,
                "pageLoad": session.timeouts.page_load_ms,
                "script": session.timeouts.script_ms
            }
        }),
    };

    Ok(WebDriverResponse::success(response))
}

pub async fn delete<R: Runtime>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;

    if sessions.delete(&session_id) {
        let last_session = sessions.is_empty();
        drop(sessions);
        if last_session {
            let app = state.app.clone();
            tauri::async_runtime::spawn(async move {
                // Leave enough time for the DELETE response to flush before
                // WebKitGTK tears down the embedded HTTP server.
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                app.exit(0);
            });
        }
        Ok(WebDriverResponse::null())
    } else {
        Err(WebDriverErrorResponse::invalid_session_id(&session_id))
    }
}
