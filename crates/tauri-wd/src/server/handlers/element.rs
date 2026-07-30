use std::path::Path as FsPath;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Deserialize;
use serde_json::json;
use tauri::Runtime;

use crate::platform::poll_implicit;
use crate::server::AppState;
use crate::server::response::{WebDriverErrorResponse, WebDriverResponse, WebDriverResult};
use crate::webdriver::locator::LocatorStrategy;

#[derive(Debug, Deserialize)]
pub struct FindElementRequest {
    pub using: String,
    pub value: String,
}

#[derive(Debug, Deserialize)]
pub struct SendKeysRequest {
    pub text: String,
}

pub async fn find<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<FindElementRequest>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;

    let strategy = LocatorStrategy::from_string(&request.using).ok_or_else(|| {
        WebDriverErrorResponse::invalid_argument(&format!(
            "Unknown locator strategy: {}",
            request.using
        ))
    })?;

    let element_ref = session.elements.store();
    let js_var = element_ref.js_ref.clone();
    let element_id = element_ref.id.clone();
    let current_window = session.current_window.clone();
    let implicit_ms = session.timeouts.implicit_ms;
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let strategy_js = strategy.to_selector_js(&request.value);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let found = poll_implicit(
        implicit_ms,
        || executor.find_element(&strategy_js, &js_var),
        |found| *found,
    )
    .await?;
    if !found {
        return Err(WebDriverErrorResponse::no_such_element());
    }

    Ok(WebDriverResponse::success(json!({
        "element-6066-11e4-a52e-4f735466cecf": element_id
    })))
}

pub async fn find_all<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<FindElementRequest>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;
    let current_window = session.current_window.clone();
    let implicit_ms = session.timeouts.implicit_ms;
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let strategy = LocatorStrategy::from_string(&request.using).ok_or_else(|| {
        WebDriverErrorResponse::invalid_argument(&format!(
            "Unknown locator strategy: {}",
            request.using
        ))
    })?;

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let strategy_js = strategy.to_selector_js_multiple(&request.value);

    let temp_prefix = format!("__wd_temp_{}_", uuid::Uuid::new_v4().simple());
    let count = poll_implicit(
        implicit_ms,
        || executor.find_elements(&strategy_js, &temp_prefix),
        |count| *count > 0,
    )
    .await?;

    let mut elements = Vec::new();
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;

    for i in 0..count {
        let element_ref = session.elements.store();
        let js_var = element_ref.js_ref.clone();
        let element_id = element_ref.id.clone();

        let copy_script = format!(
            "(function() {{ window.{js_var} = window['{temp_prefix}{i}'];  return true; }})()"
        );
        let _ = executor.evaluate_js(&copy_script).await;

        elements.push(json!({
            "element-6066-11e4-a52e-4f735466cecf": element_id
        }));
    }

    Ok(WebDriverResponse::success(elements))
}

pub async fn click<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.click_element(&js_var).await?;

    Ok(WebDriverResponse::null())
}

pub async fn clear<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    executor.clear_element(&js_var).await?;

    Ok(WebDriverResponse::null())
}

pub async fn send_keys<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
    Json(request): Json<SendKeysRequest>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let implicit_ms = session.timeouts.implicit_ms;
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let interactable = poll_implicit(
        implicit_ms,
        || executor.is_element_keyboard_interactable(&js_var),
        |interactable| *interactable,
    )
    .await?;
    if !interactable {
        return Err(WebDriverErrorResponse::element_not_interactable(
            "Element is not keyboard-interactable",
        ));
    }
    if executor.is_file_input(&js_var).await? {
        let mut files = Vec::new();
        for raw_path in request.text.split('\n').filter(|value| !value.is_empty()) {
            let path = FsPath::new(raw_path);
            if !path.is_absolute() {
                return Err(WebDriverErrorResponse::invalid_argument(
                    "File input paths must be absolute",
                ));
            }
            let metadata = std::fs::metadata(path).map_err(|error| {
                WebDriverErrorResponse::invalid_argument(&format!(
                    "File input path '{}' is not readable: {error}",
                    path.display()
                ))
            })?;
            if !metadata.is_file() {
                return Err(WebDriverErrorResponse::invalid_argument(&format!(
                    "File input path '{}' is not a regular file",
                    path.display()
                )));
            }
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    WebDriverErrorResponse::invalid_argument(
                        "File input path must have a valid UTF-8 file name",
                    )
                })?
                .to_string();
            let contents = std::fs::read(path).map_err(|error| {
                WebDriverErrorResponse::invalid_argument(&format!(
                    "File input path '{}' could not be read: {error}",
                    path.display()
                ))
            })?;
            files.push((name, BASE64_STANDARD.encode(contents)));
        }
        if files.is_empty() {
            return Err(WebDriverErrorResponse::invalid_argument(
                "At least one file input path is required",
            ));
        }
        executor.set_file_input_files(&js_var, &files).await?;
    } else {
        executor
            .send_keys_to_element(&js_var, &request.text)
            .await?;
    }

    Ok(WebDriverResponse::null())
}

pub async fn get_text<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let text = executor.get_element_text(&js_var).await?;
    Ok(WebDriverResponse::success(text))
}

pub async fn get_tag_name<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let tag_name = executor.get_element_tag_name(&js_var).await?;
    Ok(WebDriverResponse::success(tag_name))
}

pub async fn get_attribute<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id, name)): Path<(String, String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let attr = executor.get_element_attribute(&js_var, &name).await?;
    Ok(WebDriverResponse::success(attr))
}

pub async fn get_property<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id, name)): Path<(String, String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let prop = executor.get_element_property(&js_var, &name).await?;
    Ok(WebDriverResponse::success(prop))
}

pub async fn is_displayed<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let displayed = executor.is_element_displayed(&js_var).await?;
    Ok(WebDriverResponse::success(displayed))
}

pub async fn is_enabled<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let enabled = executor.is_element_enabled(&js_var).await?;
    Ok(WebDriverResponse::success(enabled))
}

pub async fn get_active<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;

    let element_ref = session.elements.store();
    let js_var = element_ref.js_ref.clone();
    let element_id = element_ref.id.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let found = executor.get_active_element(&js_var).await?;
    if !found {
        return Err(WebDriverErrorResponse::no_such_element());
    }

    Ok(WebDriverResponse::success(json!({
        "element-6066-11e4-a52e-4f735466cecf": element_id
    })))
}

pub async fn find_from_element<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, parent_element_id)): Path<(String, String)>,
    Json(request): Json<FindElementRequest>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;

    let parent_element = session
        .elements
        .get(&parent_element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;
    let parent_js_var = parent_element.js_ref.clone();

    let strategy = LocatorStrategy::from_string(&request.using).ok_or_else(|| {
        WebDriverErrorResponse::invalid_argument(&format!(
            "Unknown locator strategy: {}",
            request.using
        ))
    })?;

    let element_ref = session.elements.store();
    let js_var = element_ref.js_ref.clone();
    let element_id = element_ref.id.clone();
    let current_window = session.current_window.clone();
    let implicit_ms = session.timeouts.implicit_ms;
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let strategy_js = strategy.to_selector_js_single_from_element(&request.value);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let found = poll_implicit(
        implicit_ms,
        || executor.find_element_from_element(&parent_js_var, &strategy_js, &js_var),
        |found| *found,
    )
    .await?;
    if !found {
        return Err(WebDriverErrorResponse::no_such_element());
    }

    Ok(WebDriverResponse::success(json!({
        "element-6066-11e4-a52e-4f735466cecf": element_id
    })))
}

pub async fn find_all_from_element<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, parent_element_id)): Path<(String, String)>,
    Json(request): Json<FindElementRequest>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let parent_element = session
        .elements
        .get(&parent_element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;
    let parent_js_var = parent_element.js_ref.clone();
    let current_window = session.current_window.clone();
    let implicit_ms = session.timeouts.implicit_ms;
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let strategy = LocatorStrategy::from_string(&request.using).ok_or_else(|| {
        WebDriverErrorResponse::invalid_argument(&format!(
            "Unknown locator strategy: {}",
            request.using
        ))
    })?;

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let strategy_js = strategy.to_selector_js_from_element(&request.value);

    let temp_prefix = format!("__wd_temp_{}_", uuid::Uuid::new_v4().simple());
    let count = poll_implicit(
        implicit_ms,
        || executor.find_elements_from_element(&parent_js_var, &strategy_js, &temp_prefix),
        |count| *count > 0,
    )
    .await?;

    let mut elements = Vec::new();
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;

    for i in 0..count {
        let element_ref = session.elements.store();
        let js_var = element_ref.js_ref.clone();
        let element_id = element_ref.id.clone();

        let copy_script = format!(
            "(function() {{ window.{js_var} = window['{temp_prefix}{i}'];  return true; }})()"
        );
        let _ = executor.evaluate_js(&copy_script).await;

        elements.push(json!({
            "element-6066-11e4-a52e-4f735466cecf": element_id
        }));
    }

    Ok(WebDriverResponse::success(elements))
}

pub async fn is_selected<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let selected = executor.is_element_selected(&js_var).await?;
    Ok(WebDriverResponse::success(selected))
}

pub async fn get_css_value<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id, property_name)): Path<(String, String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let value = executor
        .get_element_css_value(&js_var, &property_name)
        .await?;
    Ok(WebDriverResponse::success(value))
}

pub async fn get_rect<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let rect = executor.get_element_rect(&js_var).await?;
    Ok(WebDriverResponse::success(json!({
        "x": rect.x,
        "y": rect.y,
        "width": rect.width,
        "height": rect.height
    })))
}

pub async fn get_computed_role<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let role = executor.get_element_computed_role(&js_var).await?;
    Ok(WebDriverResponse::success(role))
}

pub async fn get_computed_label<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let label = executor.get_element_computed_label(&js_var).await?;
    Ok(WebDriverResponse::success(label))
}

pub async fn take_screenshot<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path((session_id, element_id)): Path<(String, String)>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    let element = session
        .elements
        .get(&element_id)
        .ok_or_else(WebDriverErrorResponse::no_such_element)?;

    let js_var = element.js_ref.clone();
    let current_window = session.current_window.clone();
    let timeouts = session.timeouts.clone();
    let frame_context = session.frame_context.clone();
    drop(sessions);

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let screenshot = executor.take_element_screenshot(&js_var).await?;
    Ok(WebDriverResponse::success(screenshot))
}
