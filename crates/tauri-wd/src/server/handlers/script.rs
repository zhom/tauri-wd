use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use serde_json::Value;
use tauri::Runtime;

use crate::server::AppState;
use crate::server::response::{WebDriverResponse, WebDriverResult};

#[derive(Debug, Deserialize)]
pub struct ExecuteScriptRequest {
    pub script: String,
    #[serde(default)]
    pub args: Vec<Value>,
}

pub async fn execute_sync<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<ExecuteScriptRequest>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let result = executor
        .execute_script(&request.script, &request.args)
        .await?;
    register_returned_elements(&state, &session_id, &result).await?;
    Ok(WebDriverResponse::success(result))
}

pub async fn execute_async<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<ExecuteScriptRequest>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let result = executor
        .execute_async_script(&request.script, &request.args)
        .await?;
    register_returned_elements(&state, &session_id, &result).await?;
    Ok(WebDriverResponse::success(result))
}

async fn register_returned_elements<R: Runtime>(
    state: &AppState<R>,
    session_id: &str,
    value: &Value,
) -> Result<(), crate::server::response::WebDriverErrorResponse> {
    const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";
    const SHADOW_KEY: &str = "shadow-6066-11e4-a52e-4f735466cecf";

    fn visit(value: &Value, ids: &mut Vec<String>) {
        match value {
            Value::Array(values) => values.iter().for_each(|value| visit(value, ids)),
            Value::Object(values) => {
                if let Some(id) = values.get(ELEMENT_KEY).and_then(Value::as_str) {
                    ids.push(id.to_owned());
                }
                if let Some(id) = values.get(SHADOW_KEY).and_then(Value::as_str) {
                    ids.push(id.to_owned());
                }
                values.values().for_each(|value| visit(value, ids));
            }
            _ => {}
        }
    }

    let mut ids = Vec::new();
    visit(value, &mut ids);
    if ids.is_empty() {
        return Ok(());
    }
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(session_id)?;
    for id in ids {
        session.elements.register(&id);
    }
    Ok(())
}
