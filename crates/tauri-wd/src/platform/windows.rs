use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde_json::Value;
use tauri::{Manager, Runtime, WebviewWindow};
use tokio::sync::oneshot;
use webview2_com::Microsoft::Web::WebView2::Win32::{
    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG, COREWEBVIEW2_PRINT_ORIENTATION_LANDSCAPE,
    COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT, ICoreWebView2_7, ICoreWebView2_21,
    ICoreWebView2CapturePreviewCompletedHandler, ICoreWebView2Environment6,
    ICoreWebView2ExecuteScriptResult, ICoreWebView2PrintToPdfCompletedHandler,
    ICoreWebView2ScriptDialogOpeningEventHandler,
};
use webview2_com::{CoTaskMemPWSTR, ExecuteScriptWithResultCompletedHandler};
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::System::Com::StructuredStorage::CreateStreamOnHGlobal;
use windows::Win32::System::Com::{
    COINIT_APARTMENTTHREADED, CoInitializeEx, STATFLAG_NONAME, STREAM_SEEK_SET,
};
use windows::core::{HSTRING, Interface, PCWSTR};
use windows_core::BOOL;

use crate::platform::alert_state::{AlertState, AlertStateManager, AlertType, PendingAlert};
use crate::platform::{
    FrameId, IMPLICIT_POLL_INTERVAL, INTERNAL_COMMAND_TIMEOUT, PlatformExecutor, PrintOptions,
    extract_script_result_from_inner, wrap_script_for_frame_context,
};
use crate::server::response::WebDriverErrorResponse;
use crate::webdriver::Timeouts;

/// Serializes concurrent WebView2 ExecuteScript calls per webview window.
/// On Windows, issuing multiple concurrent ExecuteScript calls against the same
/// CoreWebView2 can cause completion handlers to be silently dropped or the
/// webview to enter an invalid state, causing script timeouts or app crashes.
/// A per-label tokio::sync::Mutex ensures only one script executes at a time.
#[derive(Default)]
pub struct ScriptExecutionLocks {
    locks: std::sync::Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl ScriptExecutionLocks {
    pub fn get(&self, label: &str) -> Arc<tokio::sync::Mutex<()>> {
        let mut m = self.locks.lock().expect("ScriptExecutionLocks poisoned");
        m.entry(label.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }
}

/// Wrapper for raw COM pointer to allow sending across threads.
/// SAFETY: The COM object must only be accessed from a COM-initialized thread.
struct SendableComPtr(*mut std::ffi::c_void);
unsafe impl Send for SendableComPtr {}

impl SendableComPtr {
    fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.0
    }
}

#[derive(Clone)]
pub struct WindowsExecutor<R: Runtime> {
    window: WebviewWindow<R>,
    timeouts: Timeouts,
    frame_context: Vec<FrameId>,
}

impl<R: Runtime> WindowsExecutor<R> {
    pub fn new(window: WebviewWindow<R>, timeouts: Timeouts, frame_context: Vec<FrameId>) -> Self {
        Self {
            window,
            timeouts,
            frame_context,
        }
    }
}

fn parse_execute_script_result(result: ICoreWebView2ExecuteScriptResult) -> Result<Value, String> {
    unsafe {
        let mut succeeded = BOOL::default();
        result
            .Succeeded(&raw mut succeeded)
            .map_err(|error| format!("Failed to read script execution status: {error:?}"))?;

        if succeeded.as_bool() {
            let mut json = windows::core::PWSTR::null();
            result
                .ResultAsJson(&raw mut json)
                .map_err(|error| format!("Failed to read script result: {error:?}"))?;
            let json = CoTaskMemPWSTR::from(json).to_string();
            return Ok(
                serde_json::from_str(&json).unwrap_or_else(|_| Value::String(json.to_string()))
            );
        }

        let exception = result
            .Exception()
            .map_err(|error| format!("Failed to read script exception: {error:?}"))?;
        let mut message = windows::core::PWSTR::null();
        exception
            .Message(&raw mut message)
            .map_err(|error| format!("Failed to read script exception message: {error:?}"))?;
        let message = CoTaskMemPWSTR::from(message).to_string();
        if message.is_empty() {
            Err("JavaScript execution failed".to_string())
        } else {
            Err(message)
        }
    }
}

impl<R: Runtime + 'static> WindowsExecutor<R> {
    /// Core WebView2 script execution — no per-webview lock.
    /// Callers that need serialization must acquire the lock from
    /// `ScriptExecutionLocks` before calling this method.
    async fn evaluate_js_inner(&self, script: &str) -> Result<Value, WebDriverErrorResponse> {
        let (tx, rx) = oneshot::channel();
        let script_preview: String = script.chars().take(100).collect();
        let script_owned = wrap_script_for_frame_context(script, &self.frame_context);

        let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

        let result = self.window.with_webview({
            let tx = tx.clone();
            move |webview| unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

                if let Ok(webview2) = webview.controller().CoreWebView2() {
                    // The legacy ExecuteScript API returns JSON null for both JavaScript
                    // exceptions and successful null values, so it cannot preserve WebDriver
                    // error semantics.
                    let webview21: ICoreWebView2_21 = match webview2.cast() {
                        Ok(webview21) => webview21,
                        Err(error) => {
                            if let Ok(mut guard) = tx.lock()
                                && let Some(tx) = guard.take()
                            {
                                let _ = tx.send(Err(format!(
                                    "WebView2 runtime does not support exception-aware script execution: {error:?}"
                                )));
                            }
                            return;
                        }
                    };
                    let script_hstring = HSTRING::from(&script_owned);
                    let handler_tx = tx.clone();

                    let handler = ExecuteScriptWithResultCompletedHandler::create(Box::new(
                        move |call, result| {
                            let response = match call {
                                Ok(()) => result
                                    .ok_or_else(|| "Script execution returned no result".to_string())
                                    .and_then(parse_execute_script_result),
                                Err(error) => Err(format!("Script execution failed: {error:?}")),
                            };

                            if let Ok(mut guard) = handler_tx.lock()
                                && let Some(tx) = guard.take()
                            {
                                let _ = tx.send(response);
                            }
                            Ok(())
                        },
                    ));

                    if let Err(e) = webview21
                        .ExecuteScriptWithResult(PCWSTR(script_hstring.as_ptr()), &handler)
                    {
                        tracing::error!(
                            "ExecuteScriptWithResult call failed for script '{}...': {e:?}",
                            script_preview
                        );
                        if let Ok(mut guard) = tx.lock()
                            && let Some(tx) = guard.take()
                        {
                            let _ =
                                tx.send(Err(format!("ExecuteScriptWithResult failed: {e:?}")));
                        }
                    }
                } else {
                    tracing::error!("Failed to get CoreWebView2 for script execution");
                    if let Ok(mut guard) = tx.lock()
                        && let Some(tx) = guard.take()
                    {
                        let _ = tx.send(Err("Failed to get CoreWebView2".to_string()));
                    }
                }
            }
        });

        if let Err(e) = result {
            tracing::error!("with_webview failed: {e}");
            if let Ok(mut guard) = tx.lock()
                && let Some(tx) = guard.take()
            {
                let _ = tx.send(Err(e.to_string()));
            }
        }

        match tokio::time::timeout(INTERNAL_COMMAND_TIMEOUT, rx).await {
            Ok(Ok(Ok(value))) => Ok(serde_json::json!({
                "success": true,
                "value": value
            })),
            Ok(Ok(Err(error))) => Err(WebDriverErrorResponse::javascript_error(&error, None)),
            Ok(Err(_)) => {
                tracing::error!("Channel closed unexpectedly during script execution");
                Err(WebDriverErrorResponse::unknown_error("Channel closed"))
            }
            Err(_) => Err(WebDriverErrorResponse::script_timeout()),
        }
    }
}

/// Register `WebView2` handlers at webview creation time.
/// This is called from the plugin's `on_webview_ready` hook to ensure
/// the script dialog handler is registered before any navigation completes.
pub fn register_webview_handlers<R: Runtime>(webview: &tauri::Webview<R>) {
    let manager = webview.app_handle().state::<AlertStateManager>();
    let alert_state = manager.get_or_create(webview.label());

    let _ = webview.with_webview(move |webview| unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

        if let Ok(webview2) = webview.controller().CoreWebView2() {
            // Disable default script dialogs so ScriptDialogOpening event fires
            if let Ok(settings) = webview2.Settings() {
                if let Err(e) = settings.SetAreDefaultScriptDialogsEnabled(false) {
                    tracing::error!("Failed to disable default script dialogs: {e:?}");
                    return;
                }
            } else {
                tracing::error!("Failed to get webview settings");
                return;
            }

            let handler: ICoreWebView2ScriptDialogOpeningEventHandler =
                ScriptDialogOpeningHandler::new(alert_state).into();

            let mut token = std::mem::zeroed();
            if let Err(e) = webview2.add_ScriptDialogOpening(&handler, &raw mut token) {
                tracing::error!("Failed to register ScriptDialogOpening handler: {e:?}");
            } else {
                tracing::debug!("Registered script dialog handler for webview");
            }

            // Prevent handler from being dropped - leak it to keep the COM ref alive
            std::mem::forget(handler);
        }
    });
}

#[async_trait]
impl<R: Runtime + 'static> PlatformExecutor<R> for WindowsExecutor<R> {
    fn window(&self) -> &WebviewWindow<R> {
        &self.window
    }

    fn script_timeout_ms(&self) -> Option<u64> {
        self.timeouts.script_ms
    }

    async fn evaluate_js(&self, script: &str) -> Result<Value, WebDriverErrorResponse> {
        let locks = self.window.state::<ScriptExecutionLocks>();
        let lock = locks.get(self.window.label());
        let _guard = lock.lock().await;
        self.evaluate_js_inner(script).await
    }

    async fn take_screenshot(&self) -> Result<String, WebDriverErrorResponse> {
        let (tx, rx) = oneshot::channel();

        let result = self.window.with_webview(move |webview| unsafe {
            if let Ok(webview2) = webview.controller().CoreWebView2() {
                let stream = match CreateStreamOnHGlobal(HGLOBAL::default(), true) {
                    Ok(s) => s,
                    Err(e) => {
                        let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
                        if let Ok(mut guard) = tx.lock()
                            && let Some(tx) = guard.take()
                        {
                            let _ = tx.send(Err(format!("Failed to create stream: {e}")));
                        }
                        return;
                    }
                };

                let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
                let handler = CapturePreviewHandler::new(tx, stream.clone());
                let handler: ICoreWebView2CapturePreviewCompletedHandler = handler.into();

                if let Err(e) = webview2.CapturePreview(
                    COREWEBVIEW2_CAPTURE_PREVIEW_IMAGE_FORMAT_PNG,
                    &stream,
                    &handler,
                ) {
                    tracing::error!("CapturePreview failed: {e}");
                }
            }
        });

        if let Err(e) = result {
            return Err(WebDriverErrorResponse::unknown_error(&e.to_string()));
        }

        match tokio::time::timeout(INTERNAL_COMMAND_TIMEOUT, rx).await {
            Ok(Ok(Ok(base64))) => {
                if base64.is_empty() {
                    Err(WebDriverErrorResponse::unknown_error(
                        "Screenshot returned empty data",
                    ))
                } else {
                    Ok(base64)
                }
            }
            Ok(Ok(Err(error))) => Err(WebDriverErrorResponse::unknown_error(&error)),
            Ok(Err(_)) => Err(WebDriverErrorResponse::unknown_error("Channel closed")),
            Err(_) => Err(WebDriverErrorResponse::script_timeout()),
        }
    }

    #[allow(clippy::too_many_lines)]
    async fn print_page(&self, options: PrintOptions) -> Result<String, WebDriverErrorResponse> {
        let (tx, rx) = oneshot::channel();
        let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

        // Create temp directory for PDF output (auto-cleanup on drop)
        // Note: We use TempDir instead of NamedTempFile because NamedTempFile
        // opens/locks the file on Windows, preventing WebView2 from writing to it
        let temp_dir = tempfile::TempDir::new().map_err(|e| {
            WebDriverErrorResponse::unknown_error(&format!("Failed to create temp dir: {e}"))
        })?;
        let pdf_path = temp_dir.path().join("print.pdf");
        let pdf_path_clone = pdf_path.clone();

        let orientation = options.orientation.clone();
        let scale = options.scale;
        let background = options.background;
        let page_width = options.page_width;
        let page_height = options.page_height;
        let margin_top = options.margin_top;
        let margin_bottom = options.margin_bottom;
        let margin_left = options.margin_left;
        let margin_right = options.margin_right;

        let result = self.window.with_webview(move |webview| unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            let webview2 = match webview.controller().CoreWebView2() {
                Ok(wv) => wv,
                Err(e) => {
                    if let Ok(mut guard) = tx.lock()
                        && let Some(tx) = guard.take()
                    {
                        let _ = tx.send(Err(format!("Failed to get CoreWebView2: {e:?}")));
                    }
                    return;
                }
            };

            // Cast to ICoreWebView2_7 for PrintToPdf support
            let webview7: ICoreWebView2_7 = match webview2.cast() {
                Ok(wv) => wv,
                Err(e) => {
                    if let Ok(mut guard) = tx.lock()
                        && let Some(tx) = guard.take()
                    {
                        let _ = tx.send(Err(format!("Failed to cast to ICoreWebView2_7: {e:?}")));
                    }
                    return;
                }
            };

            let environment = match webview7.Environment() {
                Ok(env) => env,
                Err(e) => {
                    if let Ok(mut guard) = tx.lock()
                        && let Some(tx) = guard.take()
                    {
                        let _ = tx.send(Err(format!("Failed to get environment: {e:?}")));
                    }
                    return;
                }
            };

            // Cast to ICoreWebView2Environment6 for CreatePrintSettings
            let env6: ICoreWebView2Environment6 = match environment.cast() {
                Ok(env) => env,
                Err(e) => {
                    if let Ok(mut guard) = tx.lock()
                        && let Some(tx) = guard.take()
                    {
                        let _ = tx.send(Err(format!(
                            "Failed to cast to ICoreWebView2Environment6: {e:?}"
                        )));
                    }
                    return;
                }
            };

            let settings = match env6.CreatePrintSettings() {
                Ok(s) => s,
                Err(e) => {
                    if let Ok(mut guard) = tx.lock()
                        && let Some(tx) = guard.take()
                    {
                        let _ = tx.send(Err(format!("Failed to create print settings: {e:?}")));
                    }
                    return;
                }
            };

            if let Some(ref orient) = orientation {
                let orientation_val = if orient == "landscape" {
                    COREWEBVIEW2_PRINT_ORIENTATION_LANDSCAPE
                } else {
                    COREWEBVIEW2_PRINT_ORIENTATION_PORTRAIT
                };
                let _ = settings.SetOrientation(orientation_val);
            }

            // Scale factor (1.0 = 100%)
            if let Some(s) = scale {
                let _ = settings.SetScaleFactor(s);
            }

            if let Some(bg) = background {
                let _ = settings.SetShouldPrintBackgrounds(bg);
            }

            // Page dimensions (WebDriver uses cm, WebView2 uses inches)
            // 1 inch = 2.54 cm
            if let Some(w) = page_width {
                let _ = settings.SetPageWidth(w / 2.54);
            }
            if let Some(h) = page_height {
                let _ = settings.SetPageHeight(h / 2.54);
            }

            // Margins (WebDriver uses cm, WebView2 uses inches)
            if let Some(m) = margin_top {
                let _ = settings.SetMarginTop(m / 2.54);
            }
            if let Some(m) = margin_bottom {
                let _ = settings.SetMarginBottom(m / 2.54);
            }
            if let Some(m) = margin_left {
                let _ = settings.SetMarginLeft(m / 2.54);
            }
            if let Some(m) = margin_right {
                let _ = settings.SetMarginRight(m / 2.54);
            }

            let handler: ICoreWebView2PrintToPdfCompletedHandler =
                handlers::PrintToPdfHandler::new(tx).into();

            let path_str = pdf_path_clone.to_string_lossy().to_string();
            let path_hstring = HSTRING::from(&path_str);

            if let Err(e) = webview7.PrintToPdf(&path_hstring, &settings, &handler) {
                tracing::error!("PrintToPdf call failed: {e:?}");
            }
        });

        if let Err(e) = result {
            return Err(WebDriverErrorResponse::unknown_error(&e.to_string()));
        }

        let print_result = match tokio::time::timeout(INTERNAL_COMMAND_TIMEOUT, rx).await {
            Ok(Ok(Ok(()))) => Ok(()),
            Ok(Ok(Err(error))) => Err(WebDriverErrorResponse::unknown_error(&error)),
            Ok(Err(_)) => Err(WebDriverErrorResponse::unknown_error("Channel closed")),
            Err(_) => Err(WebDriverErrorResponse::script_timeout()),
        };

        print_result?;

        let pdf_data = std::fs::read(&pdf_path).map_err(|e| {
            WebDriverErrorResponse::unknown_error(&format!("Failed to read PDF file: {e}"))
        })?;

        Ok(BASE64_STANDARD.encode(&pdf_data))
    }

    async fn execute_async_script(
        &self,
        script: &str,
        args: &[Value],
    ) -> Result<Value, WebDriverErrorResponse> {
        let args_json = serde_json::to_string(args)
            .map_err(|e| WebDriverErrorResponse::invalid_argument(&e.to_string()))?;

        let result_var = format!("__tauri_wd_async_{}", uuid::Uuid::new_v4());

        let wrapper = format!(
            r"(function() {{
                var ELEMENT_KEY = 'element-6066-11e4-a52e-4f735466cecf';
                var SHADOW_KEY = 'shadow-6066-11e4-a52e-4f735466cecf';

                function uuid() {{
                    if (globalThis.crypto && typeof globalThis.crypto.randomUUID === 'function') {{
                        return globalThis.crypto.randomUUID();
                    }}
                    return 'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, function(c) {{
                        var r = Math.random() * 16 | 0;
                        return (c === 'x' ? r : (r & 3 | 8)).toString(16);
                    }});
                }}

                function storeNode(value, key) {{
                    window.__tauriWdNodeIds = window.__tauriWdNodeIds || new WeakMap();
                    var id = window.__tauriWdNodeIds.get(value);
                    if (!id) {{
                        id = uuid();
                        window.__tauriWdNodeIds.set(value, id);
                    }}
                    window['__wd_el_' + id.replace(/-/g, '')] = value;
                    var reference = {{}};
                    reference[key] = id;
                    return reference;
                }}

                function serializeResult(value, seen) {{
                    if (value === undefined || value === null) return null;
                    if (typeof value === 'boolean') return value;
                    if (typeof value === 'number') return isFinite(value) ? value : null;
                    if (typeof value === 'string') return value;
                    if (typeof value === 'function' || typeof value === 'symbol') return null;
                    if (typeof value === 'bigint') {{
                        throw new TypeError('BigInt is not JSON serializable');
                    }}
                    if (value && value.nodeType === 1) {{
                        return storeNode(value, ELEMENT_KEY);
                    }}
                    if (value && value.nodeType === 11 && value.host) {{
                        return storeNode(value, SHADOW_KEY);
                    }}
                    if (typeof value === 'object') {{
                        if (value[ELEMENT_KEY] || value[SHADOW_KEY]) return value;
                        seen = seen || new WeakSet();
                        if (seen.has(value)) throw new TypeError('cyclic object value');
                        seen.add(value);
                        if (Array.isArray(value) ||
                            value instanceof NodeList ||
                            value instanceof HTMLCollection) {{
                            var list = Array.from(value).map(function(item) {{
                                return serializeResult(item, seen);
                            }});
                            seen.delete(value);
                            return list;
                        }}
                        var result = {{}};
                        try {{
                            for (var key in value) {{
                                if (Object.prototype.hasOwnProperty.call(value, key)) {{
                                    result[key] = serializeResult(value[key], seen);
                                }}
                            }}
                        }} finally {{
                            seen.delete(value);
                        }}
                        return result;
                    }}
                    return null;
                }}

                function deserializeArg(arg) {{
                    if (arg === null || arg === undefined) return arg;
                    if (Array.isArray(arg)) return arg.map(deserializeArg);
                    if (typeof arg === 'object') {{
                        if (arg[ELEMENT_KEY]) {{
                            var el = window['__wd_el_' + arg[ELEMENT_KEY].replace(/-/g, '')];
                            if (!el) throw new Error('stale element reference');
                            return el;
                        }}
                        var result = {{}};
                        for (var key in arg) {{
                            if (arg.hasOwnProperty(key)) result[key] = deserializeArg(arg[key]);
                        }}
                        return result;
                    }}
                    return arg;
                }}

                var __completed = false;
                var __done = function(r) {{
                    if (__completed) return;
                    try {{
                        var serialized = serializeResult(r);
                        __completed = true;
                        window['{result_var}'] = {{
                            __wd_success: true,
                            __wd_value: serialized
                        }};
                    }} catch (e) {{
                        __completed = true;
                        window['{result_var}'] = {{
                            __wd_success: false,
                            __wd_error: e.message || String(e)
                        }};
                    }}
                }};

                try {{
                    var __args = {args_json}.map(deserializeArg);
                    __args.push(__done);
                    (function() {{ {script} }}).apply(null, __args);
                }} catch (e) {{
                    if (!__completed) {{
                        __completed = true;
                        window['{result_var}'] = {{
                            __wd_success: false,
                            __wd_error: e.message || String(e)
                        }};
                    }}
                }}

                return undefined;
            }})()"
        );

        // Keep all wrapper and polling calls in one serialized WebView2 operation. Every
        // evaluation is wrapped into the selected frame, so the result remains accessible
        // even though WebView2's top-level WebMessageReceived event cannot see iframe posts.
        let locks = self.window.state::<ScriptExecutionLocks>();
        let lock = locks.get(self.window.label());
        let _guard = lock.lock().await;

        self.evaluate_js_inner(&wrapper).await?;

        let poll_script = format!("window['{result_var}']");
        let cleanup_script = format!("delete window['{result_var}']");
        let timeout = self.timeouts.script_ms.map(Duration::from_millis);
        let start = std::time::Instant::now();

        loop {
            let poll_result = self.evaluate_js_inner(&poll_script).await?;
            let inner = poll_result.get("value").cloned().unwrap_or(Value::Null);

            if !inner.is_null() && inner.get("__wd_success").is_some() {
                let _ = self.evaluate_js_inner(&cleanup_script).await;
                return extract_script_result_from_inner(&inner);
            }

            if timeout.is_some_and(|timeout| start.elapsed() >= timeout) {
                let _ = self.evaluate_js_inner(&cleanup_script).await;
                return Err(WebDriverErrorResponse::script_timeout());
            }

            tokio::time::sleep(IMPLICIT_POLL_INTERVAL).await;
        }
    }
}

type CaptureResultSender = Arc<std::sync::Mutex<Option<oneshot::Sender<Result<String, String>>>>>;
type PrintResultSender = Arc<std::sync::Mutex<Option<oneshot::Sender<Result<(), String>>>>>;

mod handlers {
    #![allow(clippy::inline_always, clippy::ref_as_ptr)]

    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT, COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM,
        COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT, ICoreWebView2,
        ICoreWebView2CapturePreviewCompletedHandler,
        ICoreWebView2CapturePreviewCompletedHandler_Impl, ICoreWebView2Deferral,
        ICoreWebView2PrintToPdfCompletedHandler, ICoreWebView2PrintToPdfCompletedHandler_Impl,
        ICoreWebView2ScriptDialogOpeningEventArgs, ICoreWebView2ScriptDialogOpeningEventHandler,
        ICoreWebView2ScriptDialogOpeningEventHandler_Impl,
    };
    use windows::core::{Interface, implement};

    use super::{
        AlertState, AlertType, CaptureResultSender, PendingAlert, PrintResultSender, SendableComPtr,
    };
    use crate::platform::alert_state::AlertResponse;
    use std::sync::Arc;

    #[implement(ICoreWebView2CapturePreviewCompletedHandler)]
    pub struct CapturePreviewHandler {
        pub tx: CaptureResultSender,
        pub stream: windows::Win32::System::Com::IStream,
    }

    impl CapturePreviewHandler {
        pub fn new(tx: CaptureResultSender, stream: windows::Win32::System::Com::IStream) -> Self {
            Self { tx, stream }
        }
    }

    impl ICoreWebView2CapturePreviewCompletedHandler_Impl for CapturePreviewHandler_Impl {
        fn Invoke(&self, errorcode: windows::core::HRESULT) -> windows::core::Result<()> {
            let response = if errorcode.is_err() {
                Err(format!("Capture preview failed: {errorcode:?}"))
            } else {
                unsafe {
                    use super::{STATFLAG_NONAME, STREAM_SEEK_SET};
                    use base64::Engine as _;
                    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;

                    let mut stat = std::mem::zeroed();
                    if self.stream.Stat(&raw mut stat, STATFLAG_NONAME).is_err() {
                        return Ok(());
                    }
                    let size = usize::try_from(stat.cbSize).unwrap_or(0);

                    if size == 0 {
                        if let Ok(mut guard) = self.tx.lock()
                            && let Some(tx) = guard.take()
                        {
                            let _ = tx.send(Err("Empty stream".to_string()));
                        }
                        return Ok(());
                    }

                    let _ = self.stream.Seek(0, STREAM_SEEK_SET, None);

                    let mut buffer = vec![0u8; size];
                    let mut bytes_read = 0u32;
                    if self
                        .stream
                        .Read(
                            buffer.as_mut_ptr().cast(),
                            u32::try_from(size).unwrap_or(u32::MAX),
                            Some(&raw mut bytes_read),
                        )
                        .is_err()
                    {
                        if let Ok(mut guard) = self.tx.lock()
                            && let Some(tx) = guard.take()
                        {
                            let _ = tx.send(Err("Failed to read stream".to_string()));
                        }
                        return Ok(());
                    }

                    buffer.truncate(bytes_read as usize);

                    let base64 = BASE64_STANDARD.encode(&buffer);

                    if let Ok(mut guard) = self.tx.lock()
                        && let Some(tx) = guard.take()
                    {
                        let _ = tx.send(Ok(base64));
                    }
                    return Ok(());
                }
            };

            if let Ok(mut guard) = self.tx.lock()
                && let Some(tx) = guard.take()
            {
                let _ = tx.send(response);
            }
            Ok(())
        }
    }

    #[implement(ICoreWebView2PrintToPdfCompletedHandler)]
    pub struct PrintToPdfHandler {
        pub tx: PrintResultSender,
    }

    impl PrintToPdfHandler {
        pub fn new(tx: PrintResultSender) -> Self {
            Self { tx }
        }
    }

    impl ICoreWebView2PrintToPdfCompletedHandler_Impl for PrintToPdfHandler_Impl {
        fn Invoke(
            &self,
            errorcode: windows::core::HRESULT,
            issuccessful: super::BOOL,
        ) -> windows::core::Result<()> {
            let response = if errorcode.is_err() {
                Err(format!("PrintToPdf failed: {errorcode:?}"))
            } else if !issuccessful.as_bool() {
                Err("PrintToPdf was not successful".to_string())
            } else {
                Ok(())
            };

            if let Ok(mut guard) = self.tx.lock()
                && let Some(tx) = guard.take()
            {
                let _ = tx.send(response);
            }
            Ok(())
        }
    }

    #[implement(ICoreWebView2ScriptDialogOpeningEventHandler)]
    pub struct ScriptDialogOpeningHandler {
        alert_state: Arc<AlertState>,
    }

    // SAFETY: Arc<AlertState> is Send + Sync
    unsafe impl Send for ScriptDialogOpeningHandler {}
    unsafe impl Sync for ScriptDialogOpeningHandler {}

    impl ScriptDialogOpeningHandler {
        pub fn new(alert_state: Arc<AlertState>) -> Self {
            Self { alert_state }
        }
    }

    impl ICoreWebView2ScriptDialogOpeningEventHandler_Impl for ScriptDialogOpeningHandler_Impl {
        fn Invoke(
            &self,
            _sender: windows::core::Ref<'_, ICoreWebView2>,
            args: windows::core::Ref<'_, ICoreWebView2ScriptDialogOpeningEventArgs>,
        ) -> windows::core::Result<()> {
            let (args_ptr, deferral_ptr, rx) = unsafe {
                let Some(args) = args.clone() else {
                    return Ok(());
                };

                let mut kind = std::mem::zeroed();
                if args.Kind(&raw mut kind).is_err() {
                    tracing::error!("Failed to get script dialog kind");
                    return Ok(());
                }

                let mut message_ptr = windows::core::PWSTR::null();
                if args.Message(&raw mut message_ptr).is_err() {
                    tracing::error!("Failed to get script dialog message");
                    return Ok(());
                }
                let message = message_ptr.to_string().unwrap_or_default();

                let mut default_text_ptr = windows::core::PWSTR::null();
                let default_text = if args.DefaultText(&raw mut default_text_ptr).is_ok() {
                    let text = default_text_ptr.to_string().unwrap_or_default();
                    if text.is_empty() { None } else { Some(text) }
                } else {
                    None
                };

                let alert_type = if kind == COREWEBVIEW2_SCRIPT_DIALOG_KIND_ALERT {
                    AlertType::Alert
                } else if kind == COREWEBVIEW2_SCRIPT_DIALOG_KIND_CONFIRM {
                    AlertType::Confirm
                } else if kind == COREWEBVIEW2_SCRIPT_DIALOG_KIND_PROMPT {
                    AlertType::Prompt
                } else {
                    // BEFOREUNLOAD or unknown - just accept it
                    let _ = args.Accept();
                    return Ok(());
                };

                tracing::debug!("Intercepted {:?} dialog: {}", alert_type, message);

                // Get deferral to handle asynchronously (avoid blocking UI thread)
                let deferral = match args.GetDeferral() {
                    Ok(d) => d,
                    Err(e) => {
                        tracing::error!("Failed to get deferral: {e:?}");
                        let _ = args.Accept();
                        return Ok(());
                    }
                };

                let (tx, rx) = std::sync::mpsc::channel::<AlertResponse>();
                self.alert_state.set_pending(PendingAlert {
                    message: message.clone(),
                    default_text: default_text.clone(),
                    alert_type,
                    responder: tx,
                });

                // Wrap COM objects for thread transfer
                let args_ptr = SendableComPtr(args.into_raw());
                let deferral_ptr = SendableComPtr(deferral.into_raw());

                (args_ptr, deferral_ptr, rx)
            };

            // Spawn thread to wait for WebDriver response (don't block UI thread)
            std::thread::spawn(move || {
                let timeout = std::time::Duration::from_secs(30);
                let response = rx.recv_timeout(timeout);

                // SAFETY: These pointers came from valid COM objects and we're
                // accessing them from a single thread. All COM method calls are unsafe.
                unsafe {
                    let args =
                        ICoreWebView2ScriptDialogOpeningEventArgs::from_raw(args_ptr.as_ptr());
                    let deferral = ICoreWebView2Deferral::from_raw(deferral_ptr.as_ptr());

                    match response {
                        Ok(AlertResponse {
                            accepted,
                            prompt_text,
                        }) => {
                            if accepted {
                                if let Some(text) = prompt_text {
                                    let result = windows::core::HSTRING::from(text.as_str());
                                    let _ =
                                        args.SetResultText(windows::core::PCWSTR(result.as_ptr()));
                                }
                                let _ = args.Accept();
                            }
                            // If not accepted, don't call Accept() - dialog returns false/null
                        }
                        Err(_) => {
                            let _ = args.Accept();
                        }
                    }

                    // Complete the deferral to let WebView2 continue
                    let _ = deferral.Complete();
                }
            });

            Ok(())
        }
    }
}

use handlers::{CapturePreviewHandler, ScriptDialogOpeningHandler};
