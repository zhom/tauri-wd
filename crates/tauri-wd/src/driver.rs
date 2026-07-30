//! WebDriver routing and session lifecycle management.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Weak},
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::State,
    http::{
        HeaderMap, HeaderName, Method, Request, Response, StatusCode,
        header::{
            AUTHORIZATION, CACHE_CONTROL, CONNECTION, CONTENT_LENGTH, CONTENT_TYPE, HOST, ORIGIN,
            PROXY_AUTHENTICATE, PROXY_AUTHORIZATION, TE, TRAILER, TRANSFER_ENCODING, UPGRADE,
        },
        uri::Authority,
    },
    response::IntoResponse,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::{Mutex, RwLock};

use crate::{
    capabilities::parse_session_request,
    config::DriverConfig,
    error::WebDriverError,
    launcher::{AppLauncher, LaunchedApp, ManagedProcess, NativeLauncher},
};

#[derive(Clone)]
struct DriverSession {
    endpoint: String,
    token: String,
    process: Arc<dyn ManagedProcess>,
    command_lock: Arc<Mutex<()>>,
}

struct DriverState {
    config: DriverConfig,
    client: reqwest::Client,
    launcher: Arc<dyn AppLauncher>,
    sessions: RwLock<HashMap<String, DriverSession>>,
    create_lock: Mutex<()>,
}

#[derive(Clone)]
pub struct Driver {
    state: Arc<DriverState>,
}

struct ForwardedResponse {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Driver {
    pub fn new(config: DriverConfig) -> Result<Self, WebDriverError> {
        Self::with_launcher(config, Arc::new(NativeLauncher))
    }

    fn with_launcher(
        config: DriverConfig,
        launcher: Arc<dyn AppLauncher>,
    ) -> Result<Self, WebDriverError> {
        validate_listener_config(&config)?;
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                WebDriverError::unknown(format!("Failed to create HTTP client: {error}"))
            })?;

        Ok(Self {
            state: Arc::new(DriverState {
                config,
                client,
                launcher,
                sessions: RwLock::new(HashMap::new()),
                create_lock: Mutex::new(()),
            }),
        })
    }

    pub fn router(&self) -> Router {
        Router::new()
            .fallback(dispatch)
            .with_state(self.state.clone())
    }

    pub async fn shutdown(&self) {
        let sessions = {
            let mut sessions = self.state.sessions.write().await;
            sessions.drain().collect::<Vec<_>>()
        };

        for (session_id, session) in sessions {
            delete_embedded_session(&self.state, &session.endpoint, &session.token, &session_id)
                .await;
            if let Err(error) = session.process.terminate().await {
                tracing::warn!("failed to stop app during shutdown: {error}");
            }
        }
    }
}

async fn dispatch(
    State(state): State<Arc<DriverState>>,
    request: Request<Body>,
) -> Result<Response<Body>, WebDriverError> {
    validate_request_boundary(&state.config, &request)?;
    let method = request.method().clone();
    let path = request.uri().path().to_owned();

    if method == Method::GET && path == "/status" {
        return Ok(status(&state).await);
    }
    if path == "/status" || path == "/session" && method != Method::POST {
        return Err(WebDriverError::unknown_method(method.as_str(), &path));
    }

    let (parts, body) = request.into_parts();
    let body = to_bytes(body, state.config.max_body_bytes)
        .await
        .map_err(|error| {
            WebDriverError::invalid_argument(format!(
                "Request body exceeds the configured limit: {error}"
            ))
        })?
        .to_vec();

    if method == Method::POST && path == "/session" {
        return create_session(&state, parts.headers, body).await;
    }

    let session_id = session_id_from_path(&path)
        .ok_or_else(|| WebDriverError::unknown_command(method.as_str(), &path))?;
    let delete = method == Method::DELETE && is_session_root(&path);

    proxy_session(
        &state,
        session_id,
        method,
        parts
            .uri
            .path_and_query()
            .map(|value| value.as_str())
            .unwrap_or(&path),
        parts.headers,
        body,
        delete,
    )
    .await
}

fn validate_listener_config(config: &DriverConfig) -> Result<(), WebDriverError> {
    if !config.host.is_loopback() {
        return Err(WebDriverError::invalid_argument(
            "The WebDriver listener must use a loopback address",
        ));
    }
    if config.port == 0 {
        return Err(WebDriverError::invalid_argument(
            "The WebDriver listener port must be resolved before creating the router",
        ));
    }
    Ok(())
}

fn validate_request_boundary(
    config: &DriverConfig,
    request: &Request<Body>,
) -> Result<(), WebDriverError> {
    if request.headers().contains_key(ORIGIN) {
        return Err(WebDriverError::invalid_argument(
            "Requests with an Origin header are not allowed",
        ));
    }

    let mut hosts = request.headers().get_all(HOST).iter();
    let header_host = hosts.next();
    if hosts.next().is_some() {
        return Err(WebDriverError::invalid_argument(
            "Request must contain exactly one Host",
        ));
    }
    let uri_host = request.uri().authority();
    if header_host.is_none() && uri_host.is_none() {
        return Err(WebDriverError::invalid_argument(
            "Request must contain a Host",
        ));
    }
    if header_host.is_some_and(|value| {
        value
            .to_str()
            .ok()
            .and_then(|value| value.parse::<Authority>().ok())
            .is_none_or(|authority| !authority_matches_listener(&authority, config))
    }) || uri_host.is_some_and(|authority| !authority_matches_listener(authority, config))
    {
        return Err(WebDriverError::invalid_argument(
            "Request Host does not match the loopback listener",
        ));
    }

    if request.method() == Method::POST
        && request.uri().path() == "/session"
        && !has_json_content_type(request.headers())
    {
        return Err(WebDriverError::unsupported_media_type(
            "POST /session requires Content-Type: application/json",
        ));
    }

    Ok(())
}

fn authority_matches_listener(authority: &Authority, config: &DriverConfig) -> bool {
    if authority.as_str().contains('@') {
        return false;
    }
    let port_matches = match authority.port_u16() {
        Some(port) => port == config.port,
        None if authority.as_str() == authority.host() => config.port == 80,
        None => false,
    };
    if !port_matches {
        return false;
    }

    let host = authority
        .host()
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or_else(|| authority.host());
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|host| host == config.host)
}

fn has_json_content_type(headers: &HeaderMap) -> bool {
    let mut values = headers.get_all(CONTENT_TYPE).iter();
    let Some(value) = values.next() else {
        return false;
    };
    if values.next().is_some() {
        return false;
    }
    value.to_str().ok().is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
    })
}

async fn status(state: &DriverState) -> Response<Body> {
    let count = state.sessions.read().await.len();
    let ready = state.config.max_sessions == 0 || count < state.config.max_sessions;
    json_response(
        StatusCode::OK,
        json!({
            "value": {
                "ready": ready,
                "message": if ready {
                    format!("ready; {count} active session(s)")
                } else {
                    format!("at capacity; {count} active session(s)")
                }
            }
        }),
    )
}

async fn create_session(
    state: &Arc<DriverState>,
    headers: HeaderMap,
    body: Vec<u8>,
) -> Result<Response<Body>, WebDriverError> {
    let _creation = state.create_lock.lock().await;
    let count = state.sessions.read().await.len();
    if state.config.max_sessions > 0 && count >= state.config.max_sessions {
        return Err(WebDriverError::session_not_created(format!(
            "Maximum number of sessions reached ({})",
            state.config.max_sessions
        )));
    }

    let request = parse_session_request(&body)?;
    let startup_timeout = request
        .launch
        .startup_timeout
        .unwrap_or(state.config.startup_timeout);
    let launched = state
        .launcher
        .launch(&request.launch, startup_timeout)
        .await?;
    let started_at = Instant::now();

    if let Err(error) = wait_for_plugin(state, &launched, startup_timeout, started_at).await {
        terminate_quietly(&launched).await;
        return Err(error);
    }

    let remaining = startup_timeout.saturating_sub(started_at.elapsed());
    if remaining.is_zero() {
        terminate_quietly(&launched).await;
        return Err(WebDriverError::session_not_created(
            "App started, but session initialization exceeded the startup timeout",
        ));
    }

    let response = match forward(
        state,
        &launched.endpoint,
        &launched.token,
        Method::POST,
        "/session",
        headers,
        request.webdriver_body,
        remaining,
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            terminate_quietly(&launched).await;
            return Err(WebDriverError::session_not_created(format!(
                "The embedded WebDriver did not create a session: {error}"
            )));
        }
    };

    if !response.status.is_success() {
        terminate_quietly(&launched).await;
        return Ok(response.into_response());
    }

    let session_id = extract_session_id(&response.body)?;
    if let Err(error) =
        wait_for_webview(state, &launched, &session_id, startup_timeout, started_at).await
    {
        delete_embedded_session(state, &launched.endpoint, &launched.token, &session_id).await;
        terminate_quietly(&launched).await;
        return Err(error);
    }

    state.sessions.write().await.insert(
        session_id.clone(),
        DriverSession {
            endpoint: launched.endpoint,
            token: launched.token,
            process: launched.process,
            command_lock: Arc::new(Mutex::new(())),
        },
    );
    watch_session(state, session_id.clone());
    tracing::info!(%session_id, "session created");

    Ok(response.into_response())
}

async fn wait_for_plugin(
    state: &DriverState,
    launched: &LaunchedApp,
    timeout: Duration,
    started_at: Instant,
) -> Result<(), WebDriverError> {
    let status_url = format!("{}/status", launched.endpoint);
    loop {
        if let Some(status) = launched.process.exit_status().await? {
            return Err(WebDriverError::session_not_created(format!(
                "App exited before WebDriver became ready ({status})"
            )));
        }

        let probe_timeout = state
            .config
            .probe_timeout
            .min(timeout.saturating_sub(started_at.elapsed()));
        if !probe_timeout.is_zero() {
            let result = state
                .client
                .get(&status_url)
                .bearer_auth(&launched.token)
                .timeout(probe_timeout)
                .send()
                .await;
            if matches!(result, Ok(response) if response.status().is_success()) {
                return Ok(());
            }
        }

        if started_at.elapsed() >= timeout {
            return Err(WebDriverError::session_not_created(format!(
                "Embedded WebDriver was not reachable within {} ms",
                timeout.as_millis()
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_for_webview(
    state: &DriverState,
    launched: &LaunchedApp,
    session_id: &str,
    timeout: Duration,
    started_at: Instant,
) -> Result<(), WebDriverError> {
    let mut last_error = None;
    loop {
        if let Some(status) = launched.process.exit_status().await? {
            return Err(WebDriverError::session_not_created(format!(
                "App exited before its JavaScript engine became responsive ({status})"
            )));
        }

        let remaining = timeout.saturating_sub(started_at.elapsed());
        if remaining.is_zero() {
            let detail = last_error
                .map(|error: WebDriverError| format!(": {error}"))
                .unwrap_or_default();
            return Err(WebDriverError::session_not_created(format!(
                "App window exists, but its JavaScript engine did not become responsive within {} ms{detail}",
                timeout.as_millis()
            )));
        }

        match probe_webview_once(
            state,
            launched,
            session_id,
            state.config.probe_timeout.min(remaining),
        )
        .await
        {
            Ok(()) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        tokio::time::sleep(Duration::from_millis(50).min(remaining)).await;
    }
}

async fn probe_webview_once(
    state: &DriverState,
    launched: &LaunchedApp,
    session_id: &str,
    timeout: Duration,
) -> Result<(), WebDriverError> {
    let path = format!("/session/{session_id}/execute/sync");
    let response = forward(
        state,
        &launched.endpoint,
        &launched.token,
        Method::POST,
        &path,
        json_headers(),
        serde_json::to_vec(&json!({
            "script": "return document.readyState;",
            "args": []
        }))
        .expect("health probe is serializable"),
        timeout,
    )
    .await
    .map_err(|error| {
        WebDriverError::session_not_created(format!(
            "App window exists, but its JavaScript engine is unresponsive: {error}"
        ))
    })?;

    if response.status.is_success() {
        Ok(())
    } else {
        let message = webdriver_message(&response.body)
            .unwrap_or_else(|| String::from_utf8_lossy(&response.body).into_owned());
        Err(WebDriverError::session_not_created(format!(
            "App window exists, but its JavaScript health probe failed: {message}"
        )))
    }
}

async fn proxy_session(
    state: &Arc<DriverState>,
    session_id: &str,
    method: Method,
    path_and_query: &str,
    headers: HeaderMap,
    body: Vec<u8>,
    delete: bool,
) -> Result<Response<Body>, WebDriverError> {
    let session = state
        .sessions
        .read()
        .await
        .get(session_id)
        .cloned()
        .ok_or_else(|| WebDriverError::invalid_session(session_id))?;
    let _command = session.command_lock.lock().await;

    if let Some(status) = session.process.exit_status().await? {
        state.sessions.write().await.remove(session_id);
        return Err(WebDriverError::unknown(format!(
            "Tauri app for session {session_id} exited unexpectedly ({status})"
        )));
    }

    let result = forward(
        state,
        &session.endpoint,
        &session.token,
        method,
        path_and_query,
        headers,
        body,
        state.config.command_timeout,
    )
    .await;

    if delete {
        state.sessions.write().await.remove(session_id);
        if let Err(error) = session.process.terminate().await {
            tracing::warn!(%session_id, "failed to stop app: {error}");
        }
        tracing::info!(%session_id, "session deleted");
    }

    result.map(ForwardedResponse::into_response)
}

#[allow(clippy::too_many_arguments)]
async fn forward(
    state: &DriverState,
    endpoint: &str,
    token: &str,
    method: Method,
    path_and_query: &str,
    headers: HeaderMap,
    body: Vec<u8>,
    timeout: Duration,
) -> Result<ForwardedResponse, WebDriverError> {
    let url = format!("{endpoint}{path_and_query}");
    let mut request = state
        .client
        .request(method, &url)
        .body(body)
        .timeout(timeout);
    let request_hop_headers = connection_headers(&headers);
    for (name, value) in &headers {
        if !is_hop_by_hop(name, &request_hop_headers) && name != AUTHORIZATION {
            request = request.header(name, value);
        }
    }
    request = request.bearer_auth(token);

    let response = request.send().await.map_err(|error| {
        if error.is_timeout() {
            WebDriverError::timeout(format!(
                "Embedded WebDriver timed out after {} ms for {path_and_query}",
                timeout.as_millis()
            ))
        } else {
            WebDriverError::unknown(format!(
                "Embedded WebDriver request failed for {path_and_query}: {error}"
            ))
        }
    })?;

    let status = response.status();
    let headers = response.headers().clone();
    if response
        .content_length()
        .is_some_and(|length| length > state.config.max_body_bytes as u64)
    {
        return Err(WebDriverError::unknown(format!(
            "Embedded WebDriver response exceeds {} bytes",
            state.config.max_body_bytes
        )));
    }
    let mut stream = response.bytes_stream();
    let mut body = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| {
            WebDriverError::unknown(format!(
                "Failed to read embedded WebDriver response for {path_and_query}: {error}"
            ))
        })?;
        if body.len().saturating_add(chunk.len()) > state.config.max_body_bytes {
            return Err(WebDriverError::unknown(format!(
                "Embedded WebDriver response exceeds {} bytes",
                state.config.max_body_bytes
            )));
        }
        body.extend_from_slice(&chunk);
    }

    Ok(ForwardedResponse {
        status,
        headers,
        body,
    })
}

async fn delete_embedded_session(
    state: &DriverState,
    endpoint: &str,
    token: &str,
    session_id: &str,
) {
    if session_id.is_empty() {
        return;
    }
    let path = format!("/session/{session_id}");
    let _ = forward(
        state,
        endpoint,
        token,
        Method::DELETE,
        &path,
        HeaderMap::new(),
        Vec::new(),
        state.config.probe_timeout,
    )
    .await;
}

async fn terminate_quietly(launched: &LaunchedApp) {
    if let Err(error) = launched.process.terminate().await {
        tracing::warn!("failed to terminate app after startup failure: {error}");
    }
}

fn extract_session_id(body: &[u8]) -> Result<String, WebDriverError> {
    let value: Value = serde_json::from_slice(body).map_err(|error| {
        WebDriverError::session_not_created(format!(
            "Embedded WebDriver returned invalid session JSON: {error}"
        ))
    })?;
    value
        .pointer("/value/sessionId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            WebDriverError::session_not_created(
                "Embedded WebDriver response did not contain value.sessionId",
            )
        })
}

fn webdriver_message(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<Value>(body)
        .ok()?
        .pointer("/value/message")?
        .as_str()
        .map(ToOwned::to_owned)
}

fn session_id_from_path(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("/session/")?;
    let session_id = rest.split('/').next()?;
    (!session_id.is_empty()).then_some(session_id)
}

fn is_session_root(path: &str) -> bool {
    let mut components = path.split('/').filter(|value| !value.is_empty());
    matches!(
        (components.next(), components.next(), components.next()),
        (Some("session"), Some(_), None)
    )
}

fn connection_headers(headers: &HeaderMap) -> HashSet<HeaderName> {
    headers
        .get_all(CONNECTION)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .filter_map(|value| HeaderName::from_bytes(value.trim().as_bytes()).ok())
        .collect()
}

fn is_hop_by_hop(name: &HeaderName, connection_headers: &HashSet<HeaderName>) -> bool {
    connection_headers.contains(name)
        || name == HOST
        || name == CONTENT_LENGTH
        || name == CONNECTION
        || name == TRANSFER_ENCODING
        || name.as_str() == "keep-alive"
        || name == PROXY_AUTHENTICATE
        || name == PROXY_AUTHORIZATION
        || name == TE
        || name == TRAILER
        || name == UPGRADE
}

fn json_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    headers
}

fn json_response(status: StatusCode, value: Value) -> Response<Body> {
    Response::builder()
        .status(status)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(CACHE_CONTROL, "no-store")
        .body(Body::from(
            serde_json::to_vec(&value).expect("JSON response is serializable"),
        ))
        .expect("valid JSON response")
}

impl ForwardedResponse {
    fn into_response(self) -> Response<Body> {
        let mut response = Response::builder().status(self.status);
        let hop_headers = connection_headers(&self.headers);
        for (name, value) in &self.headers {
            if !is_hop_by_hop(name, &hop_headers) {
                response = response.header(name, value);
            }
        }
        response = response.header(CACHE_CONTROL, "no-store");
        response
            .body(Body::from(self.body))
            .unwrap_or_else(|error| WebDriverError::unknown(error.to_string()).into_response())
    }
}

fn watch_session(state: &Arc<DriverState>, session_id: String) {
    let state: Weak<DriverState> = Arc::downgrade(state);
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let Some(state) = state.upgrade() else {
                return;
            };
            let process = {
                let sessions = state.sessions.read().await;
                let Some(session) = sessions.get(&session_id) else {
                    return;
                };
                session.process.clone()
            };
            match process.exit_status().await {
                Ok(Some(status)) => {
                    state.sessions.write().await.remove(&session_id);
                    tracing::warn!(%session_id, %status, "app exited; session reaped");
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(%session_id, "failed to inspect app process: {error}");
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use async_trait::async_trait;
    use axum::{
        Json,
        body::to_bytes,
        routing::{delete, get, post},
    };
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;
    use crate::{capabilities::LaunchOptions, launcher::AppLauncher};

    const DRIVER_HOST: &str = "localhost:4444";

    #[derive(Default)]
    struct FakeProcess {
        terminated: AtomicBool,
        exited: AtomicBool,
    }

    #[async_trait]
    impl ManagedProcess for FakeProcess {
        async fn exit_status(&self) -> Result<Option<String>, WebDriverError> {
            Ok(self
                .exited
                .load(Ordering::SeqCst)
                .then(|| "exit status: 1".to_owned()))
        }

        async fn terminate(&self) -> Result<(), WebDriverError> {
            self.terminated.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    struct FakeLauncher {
        endpoint: String,
        process: Arc<FakeProcess>,
    }

    struct LaunchProbe {
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl AppLauncher for FakeLauncher {
        async fn launch(
            &self,
            _options: &LaunchOptions,
            _startup_timeout: Duration,
        ) -> Result<LaunchedApp, WebDriverError> {
            Ok(LaunchedApp {
                endpoint: self.endpoint.clone(),
                token: "test-token".into(),
                process: self.process.clone(),
            })
        }
    }

    #[async_trait]
    impl AppLauncher for LaunchProbe {
        async fn launch(
            &self,
            _options: &LaunchOptions,
            _startup_timeout: Duration,
        ) -> Result<LaunchedApp, WebDriverError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(WebDriverError::session_not_created(
                "launch probe must not be called",
            ))
        }
    }

    async fn mock_plugin(probe_delay: Duration) -> String {
        async fn create() -> impl IntoResponse {
            Json(json!({
                "value": {
                    "sessionId": "native-session",
                    "capabilities": {"platformName": std::env::consts::OS}
                }
            }))
        }

        async fn execute(
            State(delay): State<Duration>,
            Json(body): Json<Value>,
        ) -> impl IntoResponse {
            let script = body.get("script").and_then(Value::as_str);
            if script.is_some_and(|script| script.contains("readyState")) {
                tokio::time::sleep(delay).await;
                Json(json!({"value": "complete"}))
            } else if script.is_some_and(|script| script.contains("slow-command")) {
                tokio::time::sleep(Duration::from_millis(250)).await;
                Json(json!({"value": null}))
            } else if script.is_some_and(|script| script.contains("large-result")) {
                Json(json!({"value": "x".repeat(2048)}))
            } else {
                Json(json!({"value": body.get("args").cloned().unwrap_or_default()}))
            }
        }

        async fn set_timeouts(Json(_body): Json<Value>) -> impl IntoResponse {
            Json(json!({"value": null}))
        }

        let router = Router::new()
            .route(
                "/status",
                get(|| async { Json(json!({"value": {"ready": true}})) }),
            )
            .route("/session", post(create))
            .route(
                "/session/{id}",
                delete(|| async { Json(json!({"value": null})) }),
            )
            .route("/session/{id}/execute/sync", post(execute))
            .route("/session/{id}/timeouts", post(set_timeouts))
            .with_state(probe_delay);
        let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .expect("bind mock plugin");
        let address = listener.local_addr().expect("mock address");
        tokio::spawn(async move {
            axum::serve(listener, router)
                .await
                .expect("serve mock plugin");
        });
        format!("http://{address}")
    }

    fn json_request(path: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method(Method::POST)
            .uri(path)
            .header(HOST, DRIVER_HOST)
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                serde_json::to_vec(&body).expect("serialize request"),
            ))
            .expect("request")
    }

    fn create_request() -> Request<Body> {
        json_request(
            "/session",
            json!({
                "capabilities": {
                    "alwaysMatch": {
                        "tauri:options": {"application": "mock-app"}
                    }
                }
            }),
        )
    }

    async fn driver_router(command_timeout: Duration) -> Router {
        let endpoint = mock_plugin(Duration::ZERO).await;
        Driver::with_launcher(
            DriverConfig {
                command_timeout,
                ..DriverConfig::default()
            },
            Arc::new(FakeLauncher {
                endpoint,
                process: Arc::new(FakeProcess::default()),
            }),
        )
        .expect("driver")
        .router()
    }

    async fn assert_timeout_update_keeps_outer_bound(update: Value) {
        let router = driver_router(Duration::from_millis(75)).await;
        let create = router
            .clone()
            .oneshot(create_request())
            .await
            .expect("create response");
        assert_eq!(create.status(), StatusCode::OK);

        let update = router
            .clone()
            .oneshot(json_request("/session/native-session/timeouts", update))
            .await
            .expect("timeout update response");
        assert_eq!(update.status(), StatusCode::OK);

        let response = router
            .oneshot(json_request(
                "/session/native-session/execute/sync",
                json!({"script": "return 'slow-command';", "args": []}),
            ))
            .await
            .expect("slow command response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let value = response_json(response).await;
        assert_eq!(
            value.pointer("/value/error").and_then(Value::as_str),
            Some("timeout")
        );
    }

    async fn assert_rejected_without_launch(request: Request<Body>, status: StatusCode) {
        let calls = Arc::new(AtomicUsize::new(0));
        let driver = Driver::with_launcher(
            DriverConfig::default(),
            Arc::new(LaunchProbe {
                calls: calls.clone(),
            }),
        )
        .expect("driver");
        let response = driver
            .router()
            .oneshot(request)
            .await
            .expect("rejection response");
        assert_eq!(response.status(), status);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    async fn response_json(response: Response<Body>) -> Value {
        let bytes = to_bytes(response.into_body(), 1024 * 1024)
            .await
            .expect("response bytes");
        serde_json::from_slice(&bytes).expect("response JSON")
    }

    #[tokio::test]
    async fn script_null_cannot_disable_the_outer_command_timeout() {
        assert_timeout_update_keeps_outer_bound(json!({"script": null})).await;
    }

    #[tokio::test]
    async fn finite_timeouts_cannot_extend_the_outer_command_timeout() {
        assert_timeout_update_keeps_outer_bound(json!({
            "script": 60_000,
            "pageLoad": 120_000
        }))
        .await;
    }

    #[tokio::test]
    async fn rejects_origin_before_launching_an_application() {
        let mut request = create_request();
        request
            .headers_mut()
            .insert(ORIGIN, "https://attacker.example".parse().expect("origin"));
        assert_rejected_without_launch(request, StatusCode::BAD_REQUEST).await;
    }

    #[tokio::test]
    async fn rejects_non_listener_host_before_launching_an_application() {
        let mut request = create_request();
        request
            .headers_mut()
            .insert(HOST, "attacker.example:4444".parse().expect("host header"));
        assert_rejected_without_launch(request, StatusCode::BAD_REQUEST).await;
    }

    #[tokio::test]
    async fn rejects_non_json_session_request_before_launching_an_application() {
        let mut request = create_request();
        request
            .headers_mut()
            .insert(CONTENT_TYPE, "text/plain".parse().expect("content type"));
        assert_rejected_without_launch(request, StatusCode::UNSUPPORTED_MEDIA_TYPE).await;
    }

    #[test]
    fn accepts_only_the_configured_loopback_authority_and_port() {
        let config = DriverConfig {
            port: 49_152,
            ..DriverConfig::default()
        };
        for authority in ["localhost:49152", "LOCALHOST:49152", "127.0.0.1:49152"] {
            assert!(authority_matches_listener(
                &authority.parse().expect("valid authority"),
                &config
            ));
        }
        for authority in [
            "attacker.example:49152",
            "127.0.0.1:4444",
            "[::1]:49152",
            "user@localhost:49152",
        ] {
            assert!(!authority_matches_listener(
                &authority.parse().expect("valid authority"),
                &config
            ));
        }

        let ipv6 = DriverConfig {
            host: "::1".parse().expect("IPv6 loopback"),
            port: 49_152,
            ..DriverConfig::default()
        };
        assert!(authority_matches_listener(
            &"[::1]:49152".parse().expect("valid IPv6 authority"),
            &ipv6
        ));
    }

    #[tokio::test]
    async fn proxies_nested_w3c_element_arguments_across_many_commands() {
        let endpoint = mock_plugin(Duration::ZERO).await;
        let process = Arc::new(FakeProcess::default());
        let driver = Driver::with_launcher(
            DriverConfig::default(),
            Arc::new(FakeLauncher { endpoint, process }),
        )
        .expect("driver");
        let router = driver.router();

        let create = router
            .clone()
            .oneshot(create_request())
            .await
            .expect("create response");
        assert_eq!(create.status(), StatusCode::OK);

        let element = json!({
            "outer": [{
                "element-6066-11e4-a52e-4f735466cecf": "element-id"
            }]
        });
        for _ in 0..40 {
            let request = Request::builder()
                .method(Method::POST)
                .uri("/session/native-session/execute/sync")
                .header(HOST, DRIVER_HOST)
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&json!({
                        "script": "return arguments[0];",
                        "args": [element]
                    }))
                    .expect("serialize command"),
                ))
                .expect("request");
            let response = router
                .clone()
                .oneshot(request)
                .await
                .expect("proxy response");
            assert_eq!(response.status(), StatusCode::OK);
            let value = response_json(response).await;
            assert_eq!(value.pointer("/value/0"), Some(&element));
        }
    }

    #[tokio::test]
    async fn deleting_a_session_terminates_the_app_tree() {
        let endpoint = mock_plugin(Duration::ZERO).await;
        let process = Arc::new(FakeProcess::default());
        let driver = Driver::with_launcher(
            DriverConfig::default(),
            Arc::new(FakeLauncher {
                endpoint,
                process: process.clone(),
            }),
        )
        .expect("driver");
        let router = driver.router();
        router
            .clone()
            .oneshot(create_request())
            .await
            .expect("create response");

        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::DELETE)
                    .uri("/session/native-session")
                    .header(HOST, DRIVER_HOST)
                    .body(Body::empty())
                    .expect("request"),
            )
            .await
            .expect("delete response");

        assert_eq!(response.status(), StatusCode::OK);
        assert!(process.terminated.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn rejects_an_unresponsive_webview_with_a_bounded_error() {
        let endpoint = mock_plugin(Duration::from_secs(1)).await;
        let process = Arc::new(FakeProcess::default());
        let config = DriverConfig {
            probe_timeout: Duration::from_millis(25),
            startup_timeout: Duration::from_millis(100),
            ..DriverConfig::default()
        };
        let driver = Driver::with_launcher(
            config,
            Arc::new(FakeLauncher {
                endpoint,
                process: process.clone(),
            }),
        )
        .expect("driver");

        let response = driver
            .router()
            .oneshot(create_request())
            .await
            .expect("create response");
        let status = response.status();
        let value = response_json(response).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            value.pointer("/value/error").and_then(Value::as_str),
            Some("session not created")
        );
        assert!(
            value
                .pointer("/value/message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("unresponsive"))
        );
        assert!(process.terminated.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn serializes_commands_within_a_session() {
        let endpoint = mock_plugin(Duration::from_millis(50)).await;
        let driver = Driver::with_launcher(
            DriverConfig::default(),
            Arc::new(FakeLauncher {
                endpoint,
                process: Arc::new(FakeProcess::default()),
            }),
        )
        .expect("driver");
        let router = driver.router();
        router
            .clone()
            .oneshot(create_request())
            .await
            .expect("create response");

        let command = || {
            Request::builder()
                .method(Method::POST)
                .uri("/session/native-session/execute/sync")
                .header(HOST, DRIVER_HOST)
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"script":"return document.readyState;","args":[]}"#,
                ))
                .expect("command")
        };
        let started = Instant::now();
        let (first, second) = tokio::join!(
            router.clone().oneshot(command()),
            router.clone().oneshot(command())
        );
        assert_eq!(first.expect("first").status(), StatusCode::OK);
        assert_eq!(second.expect("second").status(), StatusCode::OK);
        assert!(started.elapsed() >= Duration::from_millis(90));
    }

    #[tokio::test]
    async fn reaps_a_crashed_app_without_a_followup_command() {
        let endpoint = mock_plugin(Duration::ZERO).await;
        let process = Arc::new(FakeProcess::default());
        let config = DriverConfig {
            max_sessions: 1,
            ..DriverConfig::default()
        };
        let driver = Driver::with_launcher(
            config,
            Arc::new(FakeLauncher {
                endpoint,
                process: process.clone(),
            }),
        )
        .expect("driver");
        let router = driver.router();
        router
            .clone()
            .oneshot(create_request())
            .await
            .expect("create response");

        process.exited.store(true, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(180)).await;
        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/status")
                    .header(HOST, DRIVER_HOST)
                    .body(Body::empty())
                    .expect("status request"),
            )
            .await
            .expect("status response");
        let value = response_json(response).await;
        assert_eq!(
            value.pointer("/value/ready").and_then(Value::as_bool),
            Some(true)
        );
        assert!(
            value
                .pointer("/value/message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("0 active"))
        );
    }

    #[tokio::test]
    async fn stops_reading_an_oversized_embedded_response() {
        let endpoint = mock_plugin(Duration::ZERO).await;
        let config = DriverConfig {
            max_body_bytes: 512,
            ..DriverConfig::default()
        };
        let driver = Driver::with_launcher(
            config,
            Arc::new(FakeLauncher {
                endpoint,
                process: Arc::new(FakeProcess::default()),
            }),
        )
        .expect("driver");
        let router = driver.router();
        router
            .clone()
            .oneshot(create_request())
            .await
            .expect("create response");
        let response = router
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/session/native-session/execute/sync")
                    .header(HOST, DRIVER_HOST)
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"script":"return 'large-result';","args":[]}"#,
                    ))
                    .expect("command"),
            )
            .await
            .expect("response");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let value = response_json(response).await;
        assert!(
            value
                .pointer("/value/message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("exceeds"))
        );
    }
}
