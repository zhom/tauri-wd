use axum::{
    Json,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct WebDriverError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl WebDriverError {
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid argument",
            message: message.into(),
        }
    }

    pub fn unsupported_media_type(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            code: "invalid argument",
            message: message.into(),
        }
    }

    pub fn invalid_session(session_id: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "invalid session id",
            message: format!("No active session with id {session_id}"),
        }
    }

    pub fn session_not_created(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "session not created",
            message: message.into(),
        }
    }

    pub fn unknown_command(method: &str, path: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "unknown command",
            message: format!("No WebDriver command for {method} {path}"),
        }
    }

    pub fn unknown_method(method: &str, path: &str) -> Self {
        Self {
            status: StatusCode::METHOD_NOT_ALLOWED,
            code: "unknown method",
            message: format!("{method} is not allowed for the WebDriver command {path}"),
        }
    }

    pub fn unknown(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "unknown error",
            message: message.into(),
        }
    }

    pub fn timeout(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "timeout",
            message: message.into(),
        }
    }
}

impl IntoResponse for WebDriverError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            Json(json!({
                "value": {
                    "error": self.code,
                    "message": self.message,
                    "stacktrace": ""
                }
            })),
        )
            .into_response();
        response.headers_mut().insert(
            header::CACHE_CONTROL,
            "no-store".parse().expect("static header"),
        );
        response
    }
}
