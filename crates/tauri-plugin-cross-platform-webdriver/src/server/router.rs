use std::sync::Arc;

use axum::{
    Router,
    extract::State,
    http::{Request, StatusCode, header},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
};
use tauri::Runtime;

use super::AppState;
use super::handlers;
use super::response::WebDriverErrorResponse;

#[allow(clippy::too_many_lines)]
pub fn create_router<R: Runtime + 'static>(state: Arc<AppState<R>>, token: String) -> Router {
    Router::new()
        .route("/status", get(handlers::status::<R>))
        .route("/session", post(handlers::session::create::<R>))
        .route(
            "/session/{session_id}",
            delete(handlers::session::delete::<R>),
        )
        .route(
            "/session/{session_id}/timeouts",
            get(handlers::timeouts::get::<R>).post(handlers::timeouts::set::<R>),
        )
        .route(
            "/session/{session_id}/url",
            get(handlers::navigation::get_url::<R>).post(handlers::navigation::navigate::<R>),
        )
        .route(
            "/session/{session_id}/title",
            get(handlers::navigation::get_title::<R>),
        )
        .route(
            "/session/{session_id}/back",
            post(handlers::navigation::back::<R>),
        )
        .route(
            "/session/{session_id}/forward",
            post(handlers::navigation::forward::<R>),
        )
        .route(
            "/session/{session_id}/refresh",
            post(handlers::navigation::refresh::<R>),
        )
        .route(
            "/session/{session_id}/element",
            post(handlers::element::find::<R>),
        )
        .route(
            "/session/{session_id}/elements",
            post(handlers::element::find_all::<R>),
        )
        .route(
            "/session/{session_id}/element/active",
            get(handlers::element::get_active::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/element",
            post(handlers::element::find_from_element::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/elements",
            post(handlers::element::find_all_from_element::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/click",
            post(handlers::element::click::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/clear",
            post(handlers::element::clear::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/value",
            post(handlers::element::send_keys::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/text",
            get(handlers::element::get_text::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/name",
            get(handlers::element::get_tag_name::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/attribute/{name}",
            get(handlers::element::get_attribute::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/property/{name}",
            get(handlers::element::get_property::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/css/{property_name}",
            get(handlers::element::get_css_value::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/rect",
            get(handlers::element::get_rect::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/selected",
            get(handlers::element::is_selected::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/displayed",
            get(handlers::element::is_displayed::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/enabled",
            get(handlers::element::is_enabled::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/computedrole",
            get(handlers::element::get_computed_role::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/computedlabel",
            get(handlers::element::get_computed_label::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/screenshot",
            get(handlers::element::take_screenshot::<R>),
        )
        .route(
            "/session/{session_id}/element/{element_id}/shadow",
            get(handlers::shadow::get_shadow_root::<R>),
        )
        .route(
            "/session/{session_id}/shadow/{shadow_id}/element",
            post(handlers::shadow::find_element_in_shadow::<R>),
        )
        .route(
            "/session/{session_id}/shadow/{shadow_id}/elements",
            post(handlers::shadow::find_elements_in_shadow::<R>),
        )
        .route(
            "/session/{session_id}/execute/sync",
            post(handlers::script::execute_sync::<R>),
        )
        .route(
            "/session/{session_id}/execute/async",
            post(handlers::script::execute_async::<R>),
        )
        .route(
            "/session/{session_id}/screenshot",
            get(handlers::screenshot::take::<R>),
        )
        .route(
            "/session/{session_id}/source",
            get(handlers::document::get_source::<R>),
        )
        .route(
            "/session/{session_id}/window",
            get(handlers::window::get_window_handle::<R>)
                .post(handlers::window::switch_to_window::<R>)
                .delete(handlers::window::close_window::<R>),
        )
        .route(
            "/session/{session_id}/window/new",
            post(handlers::window::new_window::<R>),
        )
        .route(
            "/session/{session_id}/window/handles",
            get(handlers::window::get_window_handles::<R>),
        )
        .route(
            "/session/{session_id}/window/rect",
            get(handlers::window::get_rect::<R>).post(handlers::window::set_rect::<R>),
        )
        .route(
            "/session/{session_id}/window/maximize",
            post(handlers::window::maximize::<R>),
        )
        .route(
            "/session/{session_id}/window/minimize",
            post(handlers::window::minimize::<R>),
        )
        .route(
            "/session/{session_id}/window/fullscreen",
            post(handlers::window::fullscreen::<R>),
        )
        .route(
            "/session/{session_id}/frame",
            post(handlers::frame::switch_to_frame::<R>),
        )
        .route(
            "/session/{session_id}/frame/parent",
            post(handlers::frame::switch_to_parent_frame::<R>),
        )
        .route(
            "/session/{session_id}/actions",
            post(handlers::actions::perform::<R>).delete(handlers::actions::release::<R>),
        )
        .route(
            "/session/{session_id}/cookie",
            get(handlers::cookie::get_all::<R>)
                .post(handlers::cookie::add::<R>)
                .delete(handlers::cookie::delete_all::<R>),
        )
        .route(
            "/session/{session_id}/cookie/{name}",
            get(handlers::cookie::get::<R>).delete(handlers::cookie::delete::<R>),
        )
        .route(
            "/session/{session_id}/alert/dismiss",
            post(handlers::alert::dismiss::<R>),
        )
        .route(
            "/session/{session_id}/alert/accept",
            post(handlers::alert::accept::<R>),
        )
        .route(
            "/session/{session_id}/alert/text",
            get(handlers::alert::get_text::<R>).post(handlers::alert::send_text::<R>),
        )
        .route(
            "/session/{session_id}/print",
            post(handlers::print::print::<R>),
        )
        .fallback(unknown_command)
        .method_not_allowed_fallback(unknown_method)
        .layer(middleware::from_fn_with_state(token, authorize))
        .with_state(state)
}

async fn unknown_command(request: Request<axum::body::Body>) -> WebDriverErrorResponse {
    WebDriverErrorResponse::unknown_command(request.method().as_str(), request.uri().path())
}

async fn unknown_method(request: Request<axum::body::Body>) -> WebDriverErrorResponse {
    WebDriverErrorResponse::unknown_method(request.method().as_str(), request.uri().path())
}

async fn authorize(
    State(token): State<String>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let expected = format!("Bearer {token}");
    if request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == expected)
    {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::CACHE_CONTROL, "no-store")],
            axum::Json(serde_json::json!({
                "value": {
                    "error": "unknown error",
                    "message": "Missing or invalid private automation token",
                    "stacktrace": ""
                }
            })),
        )
            .into_response()
    }
}
