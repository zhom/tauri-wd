use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Map, Value, json};

use crate::error::WebDriverError;

const TAURI_OPTIONS: &str = "tauri:options";
const ALLOWED_TAURI_OPTIONS: &[&str] = &["application", "args", "env", "cwd", "startupTimeout"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchOptions {
    pub application: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: Option<PathBuf>,
    pub startup_timeout: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRequest {
    pub launch: LaunchOptions,
    /// A canonical W3C request with the selected capability candidate and no
    /// launcher-only extension data.
    pub webdriver_body: Vec<u8>,
}

/// Selects and merges one W3C capability candidate.
///
/// A candidate is compatible when it contains a valid `tauri:options` object.
pub fn parse_session_request(body: &[u8]) -> Result<SessionRequest, WebDriverError> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|error| WebDriverError::invalid_argument(format!("Invalid JSON: {error}")))?;
    let root = value
        .as_object()
        .ok_or_else(|| WebDriverError::invalid_argument("Session request must be an object"))?;
    if root.contains_key("desiredCapabilities") {
        return Err(WebDriverError::invalid_argument(
            "desiredCapabilities is not supported; use W3C capabilities",
        ));
    }

    let capabilities = root
        .get("capabilities")
        .and_then(Value::as_object)
        .ok_or_else(|| WebDriverError::invalid_argument("Missing capabilities object"))?;
    let always_match = match capabilities.get("alwaysMatch") {
        None => Map::new(),
        Some(Value::Object(value)) => value.clone(),
        Some(_) => {
            return Err(WebDriverError::invalid_argument(
                "capabilities.alwaysMatch must be an object",
            ));
        }
    };
    let first_match = match capabilities.get("firstMatch") {
        None => vec![Map::new()],
        Some(Value::Array(values)) if !values.is_empty() => values
            .iter()
            .map(|value| {
                value.as_object().cloned().ok_or_else(|| {
                    WebDriverError::invalid_argument(
                        "capabilities.firstMatch entries must be objects",
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(Value::Array(_)) => {
            return Err(WebDriverError::invalid_argument(
                "capabilities.firstMatch cannot be empty",
            ));
        }
        Some(_) => {
            return Err(WebDriverError::invalid_argument(
                "capabilities.firstMatch must be an array",
            ));
        }
    };

    let mut first_error = None;
    for candidate in first_match {
        if let Some(duplicate) = candidate.keys().find(|key| always_match.contains_key(*key)) {
            return Err(WebDriverError::invalid_argument(format!(
                "Capability {duplicate} appears in both alwaysMatch and firstMatch"
            )));
        }

        let mut merged = always_match.clone();
        merged.extend(candidate);
        let options = match merged.get(TAURI_OPTIONS) {
            Some(Value::Object(value)) => value.clone(),
            Some(_) => {
                first_error.get_or_insert_with(|| {
                    WebDriverError::invalid_argument("tauri:options must be an object")
                });
                continue;
            }
            None => continue,
        };

        let launch = match parse_launch_options(&options) {
            Ok(value) => value,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        merged.remove(TAURI_OPTIONS);
        let webdriver_body = serde_json::to_vec(&json!({
            "capabilities": {
                "alwaysMatch": merged,
                "firstMatch": [{}]
            }
        }))
        .expect("canonical capabilities are serializable");
        return Ok(SessionRequest {
            launch,
            webdriver_body,
        });
    }

    Err(first_error.unwrap_or_else(|| {
        WebDriverError::session_not_created(
            "No compatible W3C capability candidate contains tauri:options",
        )
    }))
}

fn parse_launch_options(options: &Map<String, Value>) -> Result<LaunchOptions, WebDriverError> {
    if let Some(unknown) = options
        .keys()
        .find(|key| !ALLOWED_TAURI_OPTIONS.contains(&key.as_str()))
    {
        return Err(WebDriverError::invalid_argument(format!(
            "Unknown tauri:options field: {unknown}"
        )));
    }

    let application = string_field(options, "application")
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| WebDriverError::session_not_created("Missing tauri:options.application"))?;

    let args = match options.get("args") {
        None => Vec::new(),
        Some(Value::Array(values)) => values
            .iter()
            .map(|value| {
                value.as_str().map(ToOwned::to_owned).ok_or_else(|| {
                    WebDriverError::invalid_argument("tauri:options.args must contain only strings")
                })
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => {
            return Err(WebDriverError::invalid_argument(
                "tauri:options.args must be an array",
            ));
        }
    };

    let env = match options.get("env") {
        None => BTreeMap::new(),
        Some(Value::Object(values)) => values
            .iter()
            .map(|(key, value)| {
                value
                    .as_str()
                    .map(|value| (key.clone(), value.to_owned()))
                    .ok_or_else(|| {
                        WebDriverError::invalid_argument("tauri:options.env values must be strings")
                    })
            })
            .collect::<Result<BTreeMap<_, _>, _>>()?,
        Some(_) => {
            return Err(WebDriverError::invalid_argument(
                "tauri:options.env must be an object",
            ));
        }
    };

    let cwd = match options.get("cwd") {
        None => None,
        Some(Value::String(value)) if !value.trim().is_empty() => Some(PathBuf::from(value)),
        Some(Value::String(_)) => {
            return Err(WebDriverError::invalid_argument(
                "tauri:options.cwd cannot be empty",
            ));
        }
        Some(_) => {
            return Err(WebDriverError::invalid_argument(
                "tauri:options.cwd must be a string",
            ));
        }
    };

    let startup_timeout = match options.get("startupTimeout") {
        None => None,
        Some(Value::Number(value)) => {
            let millis = value.as_u64().filter(|value| *value > 0).ok_or_else(|| {
                WebDriverError::invalid_argument(
                    "tauri:options.startupTimeout must be a positive integer",
                )
            })?;
            Some(Duration::from_millis(millis))
        }
        Some(_) => {
            return Err(WebDriverError::invalid_argument(
                "tauri:options.startupTimeout must be a positive integer",
            ));
        }
    };

    Ok(LaunchOptions {
        application: PathBuf::from(application),
        args,
        env,
        cwd,
        startup_timeout,
    })
}

fn string_field<'a>(object: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    object.get(key).and_then(Value::as_str)
}

pub fn validate_application(options: &LaunchOptions) -> Result<(), WebDriverError> {
    if !options.application.exists() {
        return Err(WebDriverError::session_not_created(format!(
            "Application does not exist: {}",
            options.application.display()
        )));
    }
    if options.application.is_dir() {
        return Err(WebDriverError::session_not_created(format!(
            "Application must be an executable, not a directory: {}",
            options.application.display()
        )));
    }
    if let Some(cwd) = &options.cwd {
        validate_directory(cwd)?;
    }
    Ok(())
}

fn validate_directory(path: &Path) -> Result<(), WebDriverError> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(WebDriverError::session_not_created(format!(
            "Working directory does not exist: {}",
            path.display()
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<SessionRequest, WebDriverError> {
        parse_session_request(&serde_json::to_vec(&value).expect("serialize fixture"))
    }

    #[test]
    fn merges_one_w3c_candidate_and_strips_launcher_options() {
        let request = parse(json!({
            "capabilities": {
                "alwaysMatch": {
                    "pageLoadStrategy": "eager"
                },
                "firstMatch": [{
                    "browserName": "webview",
                    "tauri:options": {
                        "application": "/tmp/app",
                        "args": ["--profile", "test"],
                        "env": {"MODE": "e2e"},
                        "cwd": "/tmp",
                        "startupTimeout": 1200
                    }
                }]
            }
        }))
        .expect("valid request");

        assert_eq!(request.launch.application, PathBuf::from("/tmp/app"));
        assert_eq!(request.launch.args, ["--profile", "test"]);
        assert_eq!(
            request.launch.env.get("MODE").map(String::as_str),
            Some("e2e")
        );
        assert_eq!(request.launch.cwd, Some(PathBuf::from("/tmp")));
        assert_eq!(
            request.launch.startup_timeout,
            Some(Duration::from_millis(1200))
        );
        let forwarded: Value =
            serde_json::from_slice(&request.webdriver_body).expect("forwarded JSON");
        assert!(
            forwarded
                .pointer("/capabilities/alwaysMatch/tauri:options")
                .is_none()
        );
        assert_eq!(
            forwarded.pointer("/capabilities/alwaysMatch/pageLoadStrategy"),
            Some(&json!("eager"))
        );
    }

    #[test]
    fn rejects_capability_key_collisions() {
        let error = parse(json!({
            "capabilities": {
                "alwaysMatch": {"browserName": "webview"},
                "firstMatch": [{
                    "browserName": "other",
                    "tauri:options": {"application": "app"}
                }]
            }
        }))
        .expect_err("duplicate capability");
        assert!(
            error
                .to_string()
                .contains("both alwaysMatch and firstMatch")
        );
    }

    #[test]
    fn selects_the_first_compatible_candidate() {
        let request = parse(json!({
            "capabilities": {
                "firstMatch": [
                    {"browserName": "unrelated"},
                    {"tauri:options": {"application": "app.exe"}}
                ]
            }
        }))
        .expect("valid options");
        assert_eq!(request.launch.application, PathBuf::from("app.exe"));
    }

    #[test]
    fn rejects_desired_capabilities_and_unknown_options() {
        let desired = parse(json!({
            "desiredCapabilities": {
                "tauri:options": {"application": "app"}
            },
            "capabilities": {}
        }))
        .expect_err("non-W3C request");
        assert!(desired.to_string().contains("desiredCapabilities"));

        let unknown = parse(json!({
            "capabilities": {
                "alwaysMatch": {
                    "tauri:options": {"application": "app", "binary": "old"}
                }
            }
        }))
        .expect_err("unknown option");
        assert!(unknown.to_string().contains("Unknown tauri:options field"));
    }

    #[test]
    fn rejects_non_string_environment_values() {
        let error = parse(json!({
            "capabilities": {
                "alwaysMatch": {
                    "tauri:options": {
                        "application": "app",
                        "env": {"RETRIES": 2}
                    }
                }
            }
        }))
        .expect_err("invalid options");
        assert!(error.to_string().contains("env values"));
    }
}
