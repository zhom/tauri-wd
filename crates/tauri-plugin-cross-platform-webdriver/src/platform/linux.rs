use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use glib::MainContext;
use javascriptcore::ValueExt;
use serde_json::Value;
use tauri::{Manager, Runtime, WebviewWindow};
use tokio::sync::oneshot;
use webkit2gtk::{
    PrintOperationExt, ScriptDialogType, SnapshotOptions, SnapshotRegion, WebViewExt,
};

use crate::platform::alert_state::{AlertStateManager, AlertType, PendingAlert};
use crate::platform::{
    FrameId, INTERNAL_COMMAND_TIMEOUT, PlatformExecutor, PrintOptions, await_script_timeout,
    wrap_script_for_frame_context,
};
use crate::server::response::WebDriverErrorResponse;
use crate::webdriver::Timeouts;

/// Convert a JavaScriptCore value to JSON with multiple fallback strategies.
///
/// WebKitGTK's `to_json()` can fail for certain types (functions, undefined,
/// circular refs, etc.). This function provides robust serialization:
fn js_value_to_json(js_value: &javascriptcore::Value) -> Result<Value, String> {
    if let Some(json_str) = js_value.to_json(0) {
        match serde_json::from_str::<Value>(json_str.as_str()) {
            Ok(value) => return Ok(value),
            Err(_) => return Ok(Value::String(json_str.to_string())),
        }
    }

    if js_value.is_null() || js_value.is_undefined() {
        return Ok(Value::Null);
    }

    if js_value.is_boolean() {
        return Ok(Value::Bool(js_value.to_boolean()));
    }

    if js_value.is_number() {
        let num_str = js_value.to_string();
        if let Ok(n) = num_str.parse::<f64>() {
            if n.is_nan() || n.is_infinite() {
                return Ok(Value::Null);
            }
            if n == n.trunc() && n >= i64::MIN as f64 && n <= i64::MAX as f64 {
                return Ok(Value::Number(serde_json::Number::from(n as i64)));
            }
            match serde_json::Number::from_f64(n) {
                Some(num) => return Ok(Value::Number(num)),
                None => return Ok(Value::Null),
            }
        }
        return Ok(Value::Null);
    }

    if js_value.is_string() {
        return Ok(Value::String(js_value.to_string()));
    }

    let string_repr = js_value.to_string();
    if string_repr.is_empty() {
        return Ok(Value::Null);
    }
    Ok(Value::String(string_repr))
}

/// Linux `WebKitGTK` executor
#[derive(Clone)]
pub struct LinuxExecutor<R: Runtime> {
    window: WebviewWindow<R>,
    timeouts: Timeouts,
    frame_context: Vec<FrameId>,
}

impl<R: Runtime> LinuxExecutor<R> {
    pub fn new(window: WebviewWindow<R>, timeouts: Timeouts, frame_context: Vec<FrameId>) -> Self {
        Self {
            window,
            timeouts,
            frame_context,
        }
    }
}

/// Register `WebKitGTK` handlers at webview creation time.
/// This is called from the plugin's `on_webview_ready` hook to ensure
/// the script dialog handler is registered before any navigation completes.
pub fn register_webview_handlers<R: Runtime>(webview: &tauri::Webview<R>) {
    use crate::platform::alert_state::AlertResponse;
    use webkit2gtk::WebViewExt as _;

    let manager = webview.app_handle().state::<AlertStateManager>();
    let alert_state = manager.get_or_create(webview.label());

    let _ = webview.with_webview(move |webview| {
        let webview = webview.inner().clone();
        let alert_state = alert_state.clone();

        webview.connect_script_dialog(move |_webview, dialog| {
            let dialog_type = dialog.dialog_type();
            let message = dialog.message().map(|s| s.to_string()).unwrap_or_default();

            let alert_type = match dialog_type {
                ScriptDialogType::Alert => AlertType::Alert,
                ScriptDialogType::Confirm => AlertType::Confirm,
                ScriptDialogType::Prompt => AlertType::Prompt,
                ScriptDialogType::BeforeUnloadConfirm | _ => {
                    // BEFOREUNLOAD or unknown - let default behavior handle it
                    return false;
                }
            };

            let default_text = if alert_type == AlertType::Prompt {
                dialog
                    .prompt_get_default_text()
                    .map(|s| s.to_string())
                    .filter(|s| !s.is_empty())
            } else {
                None
            };

            tracing::debug!("Intercepted {:?} dialog: {}", alert_type, message);

            let (tx, rx) = std::sync::mpsc::channel::<AlertResponse>();
            alert_state.set_pending(PendingAlert {
                message: message.clone(),
                default_text: default_text.clone(),
                alert_type,
                responder: tx,
            });

            let timeout = std::time::Duration::from_secs(30);
            let response = rx.recv_timeout(timeout);

            match response {
                Ok(AlertResponse {
                    accepted,
                    prompt_text,
                }) => {
                    if alert_type == AlertType::Confirm {
                        dialog.confirm_set_confirmed(accepted);
                    } else if alert_type == AlertType::Prompt && accepted {
                        // Only set text if accepted - when dismissed, not calling
                        // prompt_set_text() causes JavaScript to receive null
                        let text = prompt_text.or(default_text).unwrap_or_default();
                        dialog.prompt_set_text(&text);
                    }
                }
                Err(_) => {
                    if alert_type == AlertType::Confirm {
                        dialog.confirm_set_confirmed(true);
                    }
                }
            }

            true
        });

        tracing::debug!("Registered script dialog handler for webview");
    });
}

#[async_trait]
impl<R: Runtime + 'static> PlatformExecutor<R> for LinuxExecutor<R> {
    fn window(&self) -> &WebviewWindow<R> {
        &self.window
    }

    fn script_timeout_ms(&self) -> Option<u64> {
        self.timeouts.script_ms
    }

    async fn evaluate_js(&self, script: &str) -> Result<Value, WebDriverErrorResponse> {
        let (tx, rx) = oneshot::channel();
        let script_owned = wrap_script_for_frame_context(script, &self.frame_context);

        let result = self.window.with_webview(move |webview| {
            let webview = webview.inner().clone();
            let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

            let ctx = MainContext::default();
            ctx.spawn_local(async move {
                let result = webview
                    .evaluate_javascript_future(&script_owned, None, None)
                    .await;
                let response: Result<Value, String> = match result {
                    Ok(js_value) => js_value_to_json(&js_value),
                    Err(e) => Err(e.to_string()),
                };

                if let Ok(mut guard) = tx.lock() {
                    if let Some(tx) = guard.take() {
                        let _ = tx.send(response);
                    }
                }
            });
        });

        if let Err(e) = result {
            return Err(WebDriverErrorResponse::javascript_error(
                &e.to_string(),
                None,
            ));
        }

        match tokio::time::timeout(INTERNAL_COMMAND_TIMEOUT, rx).await {
            Ok(Ok(Ok(value))) => Ok(serde_json::json!({
                "success": true,
                "value": value
            })),
            Ok(Ok(Err(error))) => Err(WebDriverErrorResponse::javascript_error(&error, None)),
            Ok(Err(_)) => Err(WebDriverErrorResponse::unknown_error("Channel closed")),
            Err(_) => Err(WebDriverErrorResponse::script_timeout()),
        }
    }

    async fn take_screenshot(&self) -> Result<String, WebDriverErrorResponse> {
        let (tx, rx) = oneshot::channel();

        let result = self.window.with_webview(move |webview| {
            let webview = webview.inner().clone();
            let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

            let ctx = MainContext::default();
            ctx.spawn_local(async move {
                let result = webview
                    .snapshot_future(SnapshotRegion::Visible, SnapshotOptions::NONE)
                    .await;

                let response: Result<String, String> = match result {
                    Ok(surface) => {
                        let mut png_data: Vec<u8> = Vec::new();
                        match gtk::cairo::ImageSurface::try_from(surface) {
                            Ok(image_surface) => match image_surface.write_to_png(&mut png_data) {
                                Ok(()) => Ok(BASE64_STANDARD.encode(&png_data)),
                                Err(e) => Err(format!("Failed to write PNG: {e}")),
                            },
                            Err(e) => Err(format!("Failed to downcast to ImageSurface: {e:?}")),
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };

                if let Ok(mut guard) = tx.lock() {
                    if let Some(tx) = guard.take() {
                        let _ = tx.send(response);
                    }
                }
            });
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

    async fn print_page(&self, options: PrintOptions) -> Result<String, WebDriverErrorResponse> {
        let (tx, rx) = oneshot::channel::<Result<(), String>>();

        let temp_dir = tempfile::TempDir::new().map_err(|e| {
            WebDriverErrorResponse::unknown_error(&format!("Failed to create temp dir: {e}"))
        })?;
        let pdf_path = temp_dir.path().join("print.pdf");
        let pdf_path_clone = pdf_path.clone();

        let orientation = options.orientation.clone();
        let page_width = options.page_width;
        let page_height = options.page_height;
        let margin_top = options.margin_top;
        let margin_bottom = options.margin_bottom;
        let margin_left = options.margin_left;
        let margin_right = options.margin_right;

        let result = self.window.with_webview(move |webview| {
            let webview = webview.inner().clone();

            let print_op = webkit2gtk::PrintOperation::new(&webview);

            let page_setup = gtk::PageSetup::new();

            // Page size (cm to points: 1 cm = 28.35 points)
            let width_points = page_width.unwrap_or(21.0) * 28.35;
            let height_points = page_height.unwrap_or(29.7) * 28.35;
            let paper_size = gtk::PaperSize::new_custom(
                "custom",
                "Custom",
                width_points,
                height_points,
                gtk::Unit::Points,
            );
            page_setup.set_paper_size(&paper_size);

            if orientation.as_deref() == Some("landscape") {
                page_setup.set_orientation(gtk::PageOrientation::Landscape);
            } else {
                page_setup.set_orientation(gtk::PageOrientation::Portrait);
            }

            // Margins (cm to points)
            page_setup.set_top_margin(margin_top.unwrap_or(1.0) * 28.35, gtk::Unit::Points);
            page_setup.set_bottom_margin(margin_bottom.unwrap_or(1.0) * 28.35, gtk::Unit::Points);
            page_setup.set_left_margin(margin_left.unwrap_or(1.0) * 28.35, gtk::Unit::Points);
            page_setup.set_right_margin(margin_right.unwrap_or(1.0) * 28.35, gtk::Unit::Points);

            print_op.set_page_setup(&page_setup);

            let settings = gtk::PrintSettings::new();
            settings.set_printer("Print to File");
            settings.set(
                gtk::PRINT_SETTINGS_OUTPUT_URI,
                Some(&format!("file://{}", pdf_path_clone.display())),
            );
            settings.set(gtk::PRINT_SETTINGS_OUTPUT_FILE_FORMAT, Some("pdf"));

            print_op.set_print_settings(&settings);

            let tx = Arc::new(std::sync::Mutex::new(Some(tx)));
            print_op.connect_finished(move |_op| {
                if let Ok(mut guard) = tx.lock() {
                    if let Some(tx) = guard.take() {
                        let _ = tx.send(Ok(()));
                    }
                }
            });

            // Run print operation (silent, no dialog)
            let () = print_op.print();
        });

        if let Err(e) = result {
            return Err(WebDriverErrorResponse::unknown_error(&e.to_string()));
        }

        match tokio::time::timeout(INTERNAL_COMMAND_TIMEOUT, rx).await {
            Ok(Ok(Ok(()))) => {}
            Ok(Ok(Err(error))) => {
                return Err(WebDriverErrorResponse::unknown_error(&error));
            }
            Ok(Err(_)) => {
                return Err(WebDriverErrorResponse::unknown_error("Channel closed"));
            }
            Err(_) => {
                return Err(WebDriverErrorResponse::script_timeout());
            }
        }

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

        // Build wrapper that includes argument deserialization.
        // call_async_javascript_function_future treats the body as function statements,
        // so `return` is required — without it the function returns undefined immediately.
        let wrapper = format!(
            r"return new Promise((resolve, reject) => {{
                var ELEMENT_KEY = 'element-6066-11e4-a52e-4f735466cecf';
                function serializeResult(value) {{
                    if (value === undefined || value === null) return null;
                    if (value && value.nodeType === 1) {{
                        var id = (globalThis.crypto && crypto.randomUUID) ? crypto.randomUUID() :
                            'xxxxxxxx-xxxx-4xxx-yxxx-xxxxxxxxxxxx'.replace(/[xy]/g, function(c) {{
                                var r = Math.random() * 16 | 0;
                                return (c === 'x' ? r : (r & 3 | 8)).toString(16);
                            }});
                        window['__wd_el_' + id.replace(/-/g, '')] = value;
                        var ref = {{}}; ref[ELEMENT_KEY] = id; return ref;
                    }}
                    if (Array.isArray(value)) return value.map(serializeResult);
                    if (typeof value === 'object') {{
                        var object = {{}};
                        for (var key in value) if (Object.prototype.hasOwnProperty.call(value, key))
                            object[key] = serializeResult(value[key]);
                        return object;
                    }}
                    return value;
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
                var __done = function(result, error) {{
                    if (error) {{
                        reject(new Error(typeof error === 'string' ? error : String(error)));
                    }} else {{
                        resolve(serializeResult(result));
                    }}
                }};
                var __args = {args_json}.map(deserializeArg);
                __args.push(__done);
                try {{
                    (function() {{ {script} }}).apply(null, __args);
                }} catch (e) {{
                    reject(e);
                }}
            }})"
        );

        let (tx, rx) = oneshot::channel();

        let result = self.window.with_webview(move |webview| {
            let webview = webview.inner().clone();
            let tx = Arc::new(std::sync::Mutex::new(Some(tx)));

            let ctx = MainContext::default();
            ctx.spawn_local(async move {
                // call_async_javascript_function_future handles Promises natively
                let result = webview
                    .call_async_javascript_function_future(&wrapper, None, None, None)
                    .await;

                let response: Result<Value, String> = match result {
                    Ok(js_value) => {
                        if let Some(json_str) = js_value.to_json(0) {
                            match serde_json::from_str::<Value>(json_str.as_str()) {
                                Ok(value) => Ok(value),
                                Err(_) => Ok(Value::String(json_str.to_string())),
                            }
                        } else {
                            Ok(Value::Null)
                        }
                    }
                    Err(e) => Err(e.to_string()),
                };

                if let Ok(mut guard) = tx.lock() {
                    if let Some(tx) = guard.take() {
                        let _ = tx.send(response);
                    }
                }
            });
        });

        if let Err(e) = result {
            return Err(WebDriverErrorResponse::javascript_error(
                &e.to_string(),
                None,
            ));
        }

        match await_script_timeout(self.timeouts.script_ms, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(error))) => Err(WebDriverErrorResponse::javascript_error(&error, None)),
            Ok(Err(_)) => Err(WebDriverErrorResponse::unknown_error("Channel closed")),
            Err(_) => Err(WebDriverErrorResponse::script_timeout()),
        }
    }
}
