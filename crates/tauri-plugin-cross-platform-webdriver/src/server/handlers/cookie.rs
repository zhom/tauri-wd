use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use tauri::Runtime;

use crate::platform::Cookie;
use crate::server::AppState;
use crate::server::response::{WebDriverErrorResponse, WebDriverResponse, WebDriverResult};

#[derive(Debug, Deserialize)]
pub struct AddCookieRequest {
    pub cookie: Cookie,
}

pub async fn get_all<R: Runtime + 'static>(
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
    let cookies = executor.get_all_cookies().await?;

    Ok(WebDriverResponse::success(cookies))
}

pub async fn get<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, name)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let cookie = executor.get_cookie(&name).await?;

    match cookie {
        Some(c) => Ok(WebDriverResponse::success(c)),
        None => Err(WebDriverErrorResponse::no_such_cookie(&name)),
    }
}

pub async fn add<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<AddCookieRequest>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.add_cookie(request.cookie).await?;

    Ok(WebDriverResponse::null())
}

pub async fn delete<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, name)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.delete_cookie(&name).await?;

    Ok(WebDriverResponse::null())
}

pub async fn delete_all<R: Runtime + 'static>(
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
    executor.delete_all_cookies().await?;

    Ok(WebDriverResponse::null())
}
