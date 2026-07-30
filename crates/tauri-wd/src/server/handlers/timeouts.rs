use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use tauri::Runtime;

use crate::server::AppState;
use crate::server::response::{WebDriverErrorResponse, WebDriverResponse, WebDriverResult};

const MAX_TIMEOUT: u64 = 9_007_199_254_740_991;

#[derive(Debug, Deserialize)]
pub struct TimeoutsRequest {
    #[serde(default, deserialize_with = "deserialize_timeout_update")]
    implicit: TimeoutUpdate,
    #[serde(
        rename = "pageLoad",
        default,
        deserialize_with = "deserialize_timeout_update"
    )]
    page_load: TimeoutUpdate,
    #[serde(default, deserialize_with = "deserialize_timeout_update")]
    script: TimeoutUpdate,
}

#[derive(Debug, Default, PartialEq)]
enum TimeoutUpdate {
    #[default]
    Missing,
    Value(Value),
}

fn deserialize_timeout_update<'de, D>(deserializer: D) -> Result<TimeoutUpdate, D::Error>
where
    D: Deserializer<'de>,
{
    Value::deserialize(deserializer).map(TimeoutUpdate::Value)
}

pub async fn get<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let sessions = state.sessions.read().await;
    let session = sessions.get(&session_id)?;

    Ok(WebDriverResponse::success(json!({
        "implicit": session.timeouts.implicit_ms,
        "pageLoad": session.timeouts.page_load_ms,
        "script": session.timeouts.script_ms
    })))
}

pub async fn set<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<TimeoutsRequest>,
) -> WebDriverResult {
    let mut sessions = state.sessions.write().await;
    let session = sessions.get_mut(&session_id)?;
    let mut timeouts = session.timeouts.clone();

    if let TimeoutUpdate::Value(implicit) = request.implicit {
        timeouts.implicit_ms = parse_timeout_value(&implicit, "implicit", true)?;
    }
    if let TimeoutUpdate::Value(page_load) = request.page_load {
        timeouts.page_load_ms = parse_timeout_value(&page_load, "pageLoad", true)?;
    }
    if let TimeoutUpdate::Value(script) = request.script {
        timeouts.script_ms = parse_timeout_value(&script, "script", true)?;
    }
    session.timeouts = timeouts;

    Ok(WebDriverResponse::null())
}

pub(crate) fn parse_timeout_value(
    value: &Value,
    name: &str,
    allow_null: bool,
) -> Result<Option<u64>, WebDriverErrorResponse> {
    match value {
        Value::Null if allow_null => Ok(None),
        Value::Number(number) => {
            let timeout = number.as_u64().or_else(|| {
                number.as_f64().and_then(|value| {
                    (value.is_finite()
                        && value >= 0.0
                        && value.fract() == 0.0
                        && value <= MAX_TIMEOUT as f64)
                        .then_some(value as u64)
                })
            });
            match timeout {
                Some(timeout) if timeout <= MAX_TIMEOUT => Ok(Some(timeout)),
                _ => Err(WebDriverErrorResponse::invalid_argument(&format!(
                    "{name} timeout must be an integer from 0 to {MAX_TIMEOUT}"
                ))),
            }
        }
        _ => Err(WebDriverErrorResponse::invalid_argument(&format!(
            "{name} timeout must be an integer from 0 to {MAX_TIMEOUT}{}",
            if allow_null { " or null" } else { "" }
        ))),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::TimeoutsRequest;

    #[test]
    fn script_timeout_distinguishes_missing_null_and_number() {
        let missing: TimeoutsRequest = serde_json::from_value(json!({})).unwrap();
        assert_eq!(missing.script, super::TimeoutUpdate::Missing);

        let null: TimeoutsRequest = serde_json::from_value(json!({ "script": null })).unwrap();
        assert_eq!(
            null.script,
            super::TimeoutUpdate::Value(serde_json::Value::Null)
        );

        let number: TimeoutsRequest = serde_json::from_value(json!({ "script": 250 })).unwrap();
        assert_eq!(number.script, super::TimeoutUpdate::Value(json!(250)));
    }

    #[test]
    fn timeout_validation_accepts_null_and_rejects_values_above_maximum() {
        assert_eq!(
            super::parse_timeout_value(&json!(null), "implicit", true).unwrap(),
            None
        );
        assert!(
            super::parse_timeout_value(&json!(9_007_199_254_740_992_u64), "script", true).is_err()
        );
        assert_eq!(
            super::parse_timeout_value(&json!(null), "script", true).unwrap(),
            None
        );
        assert_eq!(
            super::parse_timeout_value(&json!(2.0), "implicit", true).unwrap(),
            Some(2)
        );
    }
}
