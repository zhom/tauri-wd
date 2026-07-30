use std::{io, process::Stdio, sync::Arc, time::Duration};

use async_trait::async_trait;
use serde::Deserialize;
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, BufReader},
    sync::Mutex,
};
use uuid::Uuid;

use crate::{
    capabilities::{LaunchOptions, validate_application},
    error::WebDriverError,
};

#[derive(Clone)]
pub struct LaunchedApp {
    pub endpoint: String,
    pub token: String,
    pub process: Arc<dyn ManagedProcess>,
}

#[async_trait]
pub trait AppLauncher: Send + Sync {
    async fn launch(
        &self,
        options: &LaunchOptions,
        startup_timeout: Duration,
    ) -> Result<LaunchedApp, WebDriverError>;
}

#[async_trait]
pub trait ManagedProcess: Send + Sync {
    async fn exit_status(&self) -> Result<Option<String>, WebDriverError>;
    async fn terminate(&self) -> Result<(), WebDriverError>;
}

#[derive(Debug, Default)]
pub struct NativeLauncher;

#[cfg(any(unix, windows))]
struct NativeProcess {
    child: Mutex<Box<dyn process_wrap::tokio::ChildWrapper>>,
    _session_dir: TempDir,
}

#[cfg(any(unix, windows))]
#[async_trait]
impl ManagedProcess for NativeProcess {
    async fn exit_status(&self) -> Result<Option<String>, WebDriverError> {
        let mut child = self.child.lock().await;
        child
            .try_wait()
            .map(|status| status.map(|status| status.to_string()))
            .map_err(|error| {
                WebDriverError::unknown(format!("Failed to inspect app process: {error}"))
            })
    }

    async fn terminate(&self) -> Result<(), WebDriverError> {
        // Deleting the embedded session asks Tauri to exit normally. Give the
        // event loop a brief chance to flush app state before killing the tree.
        for _ in 0..40 {
            {
                let mut child = self.child.lock().await;
                if child.try_wait().map_err(process_error)?.is_some() {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        let mut child = self.child.lock().await;
        if child.try_wait().map_err(process_error)?.is_none() {
            Box::into_pin(child.kill()).await.map_err(process_error)?;
        }
        Ok(())
    }
}

#[cfg(any(unix, windows))]
fn process_error(error: io::Error) -> WebDriverError {
    WebDriverError::unknown(format!("Failed to terminate app process tree: {error}"))
}

#[derive(Debug, Deserialize)]
struct ReadyMessage {
    port: Option<u16>,
    error: Option<String>,
}

#[async_trait]
impl AppLauncher for NativeLauncher {
    #[cfg(any(unix, windows))]
    async fn launch(
        &self,
        options: &LaunchOptions,
        startup_timeout: Duration,
    ) -> Result<LaunchedApp, WebDriverError> {
        use process_wrap::tokio::{CommandWrap, KillOnDrop};

        validate_application(options)?;
        let session_dir = tempfile::Builder::new()
            .prefix("tauri-wd-")
            .tempdir()
            .map_err(|error| {
                WebDriverError::session_not_created(format!(
                    "Failed to create an isolated session directory: {error}"
                ))
            })?;
        let ready_file = session_dir.path().join("webdriver-ready.json");
        let profile_dir = session_dir.path().join("profile");
        std::fs::create_dir(&profile_dir).map_err(|error| {
            WebDriverError::session_not_created(format!(
                "Failed to create the isolated app profile: {error}"
            ))
        })?;
        let token = Uuid::new_v4().to_string();
        let log_id = token[..8].to_owned();
        let application = options.application.clone();
        let args = options.args.clone();
        let env = options.env.clone();
        let cwd = options.cwd.clone();
        let ready_file_env = ready_file.clone();
        let profile_dir_env = profile_dir.clone();
        let token_env = token.clone();
        let startup_timeout_ms = startup_timeout.as_millis().to_string();

        let mut command = CommandWrap::with_new(application.as_os_str(), move |command| {
            command
                .args(args)
                .envs(env)
                .env("TAURI_AUTOMATION", "true")
                .env("TAURI_WEBVIEW_AUTOMATION", "true")
                .env("TAURI_WEBDRIVER_PORT", "0")
                .env("TAURI_WEBDRIVER_TOKEN", token_env)
                .env("TAURI_WEBDRIVER_READY_FILE", ready_file_env)
                .env("TAURI_AUTOMATION_PROFILE_DIR", profile_dir_env)
                .env("TAURI_WEBDRIVER_STARTUP_TIMEOUT_MS", startup_timeout_ms)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            if let Some(cwd) = cwd {
                command.current_dir(cwd);
            }
        });

        command.wrap(KillOnDrop);
        #[cfg(unix)]
        command.wrap(process_wrap::tokio::ProcessGroup::leader());
        #[cfg(windows)]
        command.wrap(process_wrap::tokio::JobObject);

        let mut child = command.spawn().map_err(|error| {
            WebDriverError::session_not_created(format!(
                "Failed to launch {}: {error}",
                options.application.display()
            ))
        })?;

        if let Some(stdout) = child.stdout().take() {
            spawn_output_forwarder(log_id.clone(), "stdout", stdout);
        }
        if let Some(stderr) = child.stderr().take() {
            spawn_output_forwarder(log_id, "stderr", stderr);
        }

        let process: Arc<dyn ManagedProcess> = Arc::new(NativeProcess {
            child: Mutex::new(child),
            _session_dir: session_dir,
        });
        let started = tokio::time::Instant::now();
        loop {
            if let Some(status) = process.exit_status().await? {
                return Err(WebDriverError::session_not_created(format!(
                    "App exited before its WebDriver server was ready ({status})"
                )));
            }
            if let Ok(contents) = tokio::fs::read_to_string(&ready_file).await {
                let ready: ReadyMessage = serde_json::from_str(&contents).map_err(|error| {
                    WebDriverError::session_not_created(format!(
                        "App published invalid WebDriver readiness data: {error}"
                    ))
                })?;
                if let Some(error) = ready.error {
                    return Err(WebDriverError::session_not_created(format!(
                        "Embedded WebDriver failed to start: {error}"
                    )));
                }
                if let Some(port) = ready.port.filter(|port| *port > 0) {
                    return Ok(LaunchedApp {
                        endpoint: format!("http://127.0.0.1:{port}"),
                        token,
                        process,
                    });
                }
            }
            if started.elapsed() >= startup_timeout {
                let _ = process.terminate().await;
                return Err(WebDriverError::session_not_created(format!(
                    "App did not publish WebDriver readiness within {} ms",
                    startup_timeout.as_millis()
                )));
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    #[cfg(not(any(unix, windows)))]
    async fn launch(
        &self,
        _options: &LaunchOptions,
        _startup_timeout: Duration,
    ) -> Result<LaunchedApp, WebDriverError> {
        Err(WebDriverError::session_not_created(
            "This platform cannot launch desktop Tauri applications",
        ))
    }
}

fn spawn_output_forwarder<R>(app: String, stream: &'static str, reader: R)
where
    R: AsyncRead + Unpin + Send + 'static,
{
    tokio::spawn(async move {
        let mut lines = BufReader::new(reader).lines();
        loop {
            match lines.next_line().await {
                Ok(Some(line)) => {
                    if stream == "stderr" {
                        tracing::warn!(target: "tauri_app", %app, %stream, "{line}");
                    } else {
                        tracing::info!(target: "tauri_app", %app, %stream, "{line}");
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(
                        target: "tauri_app",
                        %app,
                        %stream,
                        "failed to read app output: {error}"
                    );
                    break;
                }
            }
        }
    });
}
