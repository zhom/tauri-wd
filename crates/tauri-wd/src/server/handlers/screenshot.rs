use std::sync::Arc;

use axum::extract::{Path, State};
use tauri::Runtime;

use crate::server::AppState;
use crate::server::response::{WebDriverResponse, WebDriverResult};

pub async fn take<R: Runtime + 'static>(
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
    let screenshot = executor.take_screenshot().await?;
    Ok(WebDriverResponse::success(screenshot))
}
