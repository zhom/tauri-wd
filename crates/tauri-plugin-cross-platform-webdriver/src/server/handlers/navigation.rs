use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use tauri::Runtime;

use crate::platform::PlatformExecutor;
use crate::server::AppState;
use crate::server::response::{WebDriverResponse, WebDriverResult};
use crate::webdriver::{ActionState, PageLoadStrategy};

#[derive(Debug, Deserialize)]
pub struct NavigateRequest {
    pub url: String,
}

pub async fn navigate<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<NavigateRequest>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let page_load_timeout = timeouts.page_load_ms;
    let page_load_strategy = session.page_load_strategy;
    let frame_context = session.frame_context.clone();
    session.action_state = ActionState::default();
    session.frame_context.clear();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.navigate(&request.url).await?;
    wait_for_navigation(&executor, page_load_strategy, page_load_timeout).await?;

    Ok(WebDriverResponse::null())
}

pub async fn get_url<R: Runtime + 'static>(
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
    let url = executor.get_url().await?;
    Ok(WebDriverResponse::success(url))
}

pub async fn get_title<R: Runtime + 'static>(
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
    let title = executor.get_title().await?;
    Ok(WebDriverResponse::success(title))
}

pub async fn back<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let page_load_timeout = timeouts.page_load_ms;
    let page_load_strategy = session.page_load_strategy;
    let frame_context = session.frame_context.clone();
    session.action_state = ActionState::default();
    session.frame_context.clear();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.go_back().await?;
    wait_for_navigation(&executor, page_load_strategy, page_load_timeout).await?;
    Ok(WebDriverResponse::null())
}

pub async fn forward<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let page_load_timeout = timeouts.page_load_ms;
    let page_load_strategy = session.page_load_strategy;
    let frame_context = session.frame_context.clone();
    session.action_state = ActionState::default();
    session.frame_context.clear();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.go_forward().await?;
    wait_for_navigation(&executor, page_load_strategy, page_load_timeout).await?;
    Ok(WebDriverResponse::null())
}

pub async fn refresh<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let page_load_timeout = timeouts.page_load_ms;
    let page_load_strategy = session.page_load_strategy;
    let frame_context = session.frame_context.clone();
    session.action_state = ActionState::default();
    session.frame_context.clear();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.refresh().await?;
    wait_for_navigation(&executor, page_load_strategy, page_load_timeout).await?;
    Ok(WebDriverResponse::null())
}

async fn wait_for_navigation<R: Runtime>(
    executor: &std::sync::Arc<dyn PlatformExecutor<R>>,
    strategy: PageLoadStrategy,
    timeout_ms: Option<u64>,
) -> Result<(), crate::server::response::WebDriverErrorResponse> {
    if matches!(strategy, PageLoadStrategy::None) {
        return Ok(());
    }

    let started = std::time::Instant::now();
    let timeout = timeout_ms.map(std::time::Duration::from_millis);
    loop {
        if let Ok(result) = executor
            .evaluate_js("(function(){return document.readyState;})()")
            .await
        {
            let ready_state = result.get("value").and_then(serde_json::Value::as_str);
            let ready = match strategy {
                PageLoadStrategy::None => true,
                PageLoadStrategy::Eager => matches!(ready_state, Some("interactive" | "complete")),
                PageLoadStrategy::Normal => ready_state == Some("complete"),
            };
            if ready {
                return Ok(());
            }
        }

        if timeout.is_some_and(|timeout| started.elapsed() >= timeout) {
            return Err(crate::server::response::WebDriverErrorResponse::timeout(
                "Navigation did not reach the requested document readiness state",
            ));
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}
