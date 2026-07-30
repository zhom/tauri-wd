use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::json;
use tauri::{Manager, Runtime};

use crate::platform::WindowRect;
use crate::server::AppState;
use crate::server::response::{WebDriverErrorResponse, WebDriverResponse, WebDriverResult};

#[derive(Debug, Deserialize)]
pub struct SwitchWindowRequest {
    pub handle: String,
}

#[derive(Debug, Deserialize)]
pub struct NewWindowRequest {
    #[serde(rename = "type", default)]
    pub _window_type: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WindowRectRequest {
    #[serde(default)]
    pub x: Option<i32>,
    #[serde(default)]
    pub y: Option<i32>,
    #[serde(default)]
    pub width: Option<u32>,
    #[serde(default)]
    pub height: Option<u32>,
}

pub async fn get_window_handle<R: Runtime>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    drop(sessions);

    Ok(WebDriverResponse::success(current_window))
}

pub async fn get_window_handles<R: Runtime>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let _session = sessions.get(&session_id)?;
    drop(sessions);

    let handles: Vec<String> = state.app.webview_windows().keys().cloned().collect();

    Ok(WebDriverResponse::success(handles))
}

pub async fn close_window<R: Runtime>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    drop(sessions);

    if let Some(window) = state.app.webview_windows().get(&current_window).cloned() {
        window
            .destroy()
            .map_err(|e| WebDriverErrorResponse::unknown_error(&e.to_string()))?;

        let handles: Vec<String> = state.app.webview_windows().keys().cloned().collect();

        Ok(WebDriverResponse::success(handles))
    } else {
        Err(WebDriverErrorResponse::no_such_window())
    }
}

pub async fn switch_to_window<R: Runtime>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<SwitchWindowRequest>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;

    if !state.app.webview_windows().contains_key(&request.handle) {
        return Err(WebDriverErrorResponse::no_such_window());
    }

    session.current_window = request.handle;

    Ok(WebDriverResponse::null())
}

pub async fn new_window<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(_request): Json<NewWindowRequest>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let _session = sessions.get(&session_id)?;
    drop(sessions);

    // Tauri applications own window construction and configuration.
    Err(WebDriverErrorResponse::unsupported_operation(
        "Creating new windows is not supported in this context",
    ))
}

pub async fn get_rect<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let rect = executor.get_window_rect().await?;

    Ok(WebDriverResponse::success(json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height
    })))
}

pub async fn set_rect<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<WindowRectRequest>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;

    let current = executor.get_window_rect().await?;

    let new_rect = WindowRect {
        x: request.x.unwrap_or(current.x),
        y: request.y.unwrap_or(current.y),
        width: request.width.unwrap_or(current.width),
        height: request.height.unwrap_or(current.height),
    };

    let rect = executor.set_window_rect(new_rect).await?;

    Ok(WebDriverResponse::success(json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height
    })))
}

pub async fn maximize<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let rect = executor.maximize_window().await?;

    Ok(WebDriverResponse::success(json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height
    })))
}

pub async fn minimize<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.minimize_window().await?;

    // Return null per W3C spec (minimized window has no meaningful rect)
    Ok(WebDriverResponse::null())
}

pub async fn fullscreen<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let rect = executor.fullscreen_window().await?;

    Ok(WebDriverResponse::success(json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height
    })))
}
