use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use image::ImageFormat;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::future::Future;
use std::io::Cursor;
use std::time::Duration;
use tauri::webview::Cookie as TauriCookie;
use tauri::{Runtime, WebviewWindow};

use tauri::{LogicalPosition, LogicalSize};

use tauri::Manager;

use crate::platform::alert_state::{AlertStateManager, AlertType};
use crate::server::response::WebDriverErrorResponse;

pub const INTERNAL_COMMAND_TIMEOUT: Duration = Duration::from_secs(30);
const IMPLICIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub async fn await_script_timeout<F, T>(timeout_ms: Option<u64>, future: F) -> Result<T, ()>
where
    F: Future<Output = T>,
{
    match timeout_ms {
        Some(timeout_ms) => tokio::time::timeout(Duration::from_millis(timeout_ms), future)
            .await
            .map_err(|_| ()),
        None => Ok(future.await),
    }
}

pub async fn poll_implicit<F, Fut, T, E, P>(
    timeout_ms: Option<u64>,
    mut operation: F,
    is_found: P,
) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
    P: Fn(&T) -> bool,
{
    let timeout = timeout_ms.map(Duration::from_millis);
    let start = tokio::time::Instant::now();

    loop {
        let result = operation().await?;
        if is_found(&result) || timeout.is_some_and(|timeout| start.elapsed() >= timeout) {
            return Ok(result);
        }

        let delay = timeout.map_or(IMPLICIT_POLL_INTERVAL, |timeout| {
            IMPLICIT_POLL_INTERVAL.min(timeout.saturating_sub(start.elapsed()))
        });
        tokio::time::sleep(delay).await;
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ElementRect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ScreenshotRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    viewport_width: f64,
    viewport_height: f64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WindowRect {
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone)]
pub enum FrameId {
    Index(u32),
    Element(String),
}

#[derive(Debug, Clone, Copy)]
pub enum PointerEventType {
    Down,
    Up,
    Move,
    Click,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, rename = "httpOnly")]
    pub http_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expiry: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "sameSite")]
    pub same_site: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrintOptions {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orientation: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scale: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub background: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "pageWidth")]
    pub page_width: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "pageHeight")]
    pub page_height: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "marginTop")]
    pub margin_top: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "marginBottom")]
    pub margin_bottom: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "marginLeft")]
    pub margin_left: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "marginRight")]
    pub margin_right: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "shrinkToFit")]
    pub shrink_to_fit: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "pageRanges")]
    pub page_ranges: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Default)]
#[allow(clippy::struct_excessive_bools)]
pub struct ModifierState {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub meta: bool,
}

impl ModifierState {
    pub fn update(&mut self, key: &str, is_down: bool) {
        match key {
            "\u{E009}" | "\u{E051}" => self.ctrl = is_down,
            "\u{E008}" | "\u{E050}" => self.shift = is_down,
            "\u{E00A}" | "\u{E052}" => self.alt = is_down,
            "\u{E03D}" | "\u{E053}" => self.meta = is_down,
            _ => {}
        }
    }
}

#[derive(Clone, Copy)]
struct WebDriverKeyEvent {
    key: &'static str,
    code: &'static str,
    key_code: u32,
    location: u32,
}

const fn key_event(
    key: &'static str,
    code: &'static str,
    key_code: u32,
    location: u32,
) -> WebDriverKeyEvent {
    WebDriverKeyEvent {
        key,
        code,
        key_code,
        location,
    }
}

fn webdriver_key_event(value: &str) -> Option<WebDriverKeyEvent> {
    Some(match value {
        "\u{E000}" => key_event("Unidentified", "", 0, 0),
        "\u{E001}" => key_event("Cancel", "", 3, 0),
        "\u{E002}" => key_event("Help", "Help", 6, 0),
        "\u{E003}" => key_event("Backspace", "Backspace", 8, 0),
        "\u{E004}" => key_event("Tab", "Tab", 9, 0),
        "\u{E005}" => key_event("Clear", "", 12, 0),
        "\u{E006}" => key_event("Enter", "Enter", 13, 0),
        "\u{E007}" => key_event("Enter", "NumpadEnter", 13, 1),
        "\u{E008}" => key_event("Shift", "ShiftLeft", 16, 1),
        "\u{E009}" => key_event("Control", "ControlLeft", 17, 1),
        "\u{E00A}" => key_event("Alt", "AltLeft", 18, 1),
        "\u{E00B}" => key_event("Pause", "Pause", 19, 0),
        "\u{E00C}" => key_event("Escape", "Escape", 27, 0),
        "\u{E00D}" => key_event(" ", "Space", 32, 0),
        "\u{E00E}" => key_event("PageUp", "PageUp", 33, 0),
        "\u{E00F}" => key_event("PageDown", "PageDown", 34, 0),
        "\u{E010}" => key_event("End", "End", 35, 0),
        "\u{E011}" => key_event("Home", "Home", 36, 0),
        "\u{E012}" => key_event("ArrowLeft", "ArrowLeft", 37, 0),
        "\u{E013}" => key_event("ArrowUp", "ArrowUp", 38, 0),
        "\u{E014}" => key_event("ArrowRight", "ArrowRight", 39, 0),
        "\u{E015}" => key_event("ArrowDown", "ArrowDown", 40, 0),
        "\u{E016}" => key_event("Insert", "Insert", 45, 0),
        "\u{E017}" => key_event("Delete", "Delete", 46, 0),
        "\u{E018}" => key_event(";", "", 186, 0),
        "\u{E019}" => key_event("=", "NumpadEqual", 187, 3),
        "\u{E01A}" => key_event("0", "Numpad0", 96, 3),
        "\u{E01B}" => key_event("1", "Numpad1", 97, 3),
        "\u{E01C}" => key_event("2", "Numpad2", 98, 3),
        "\u{E01D}" => key_event("3", "Numpad3", 99, 3),
        "\u{E01E}" => key_event("4", "Numpad4", 100, 3),
        "\u{E01F}" => key_event("5", "Numpad5", 101, 3),
        "\u{E020}" => key_event("6", "Numpad6", 102, 3),
        "\u{E021}" => key_event("7", "Numpad7", 103, 3),
        "\u{E022}" => key_event("8", "Numpad8", 104, 3),
        "\u{E023}" => key_event("9", "Numpad9", 105, 3),
        "\u{E024}" => key_event("*", "NumpadMultiply", 106, 3),
        "\u{E025}" => key_event("+", "NumpadAdd", 107, 3),
        "\u{E026}" => key_event(",", "NumpadComma", 108, 3),
        "\u{E027}" => key_event("-", "NumpadSubtract", 109, 3),
        "\u{E028}" => key_event(".", "NumpadDecimal", 110, 3),
        "\u{E029}" => key_event("/", "NumpadDivide", 111, 3),
        "\u{E031}" => key_event("F1", "F1", 112, 0),
        "\u{E032}" => key_event("F2", "F2", 113, 0),
        "\u{E033}" => key_event("F3", "F3", 114, 0),
        "\u{E034}" => key_event("F4", "F4", 115, 0),
        "\u{E035}" => key_event("F5", "F5", 116, 0),
        "\u{E036}" => key_event("F6", "F6", 117, 0),
        "\u{E037}" => key_event("F7", "F7", 118, 0),
        "\u{E038}" => key_event("F8", "F8", 119, 0),
        "\u{E039}" => key_event("F9", "F9", 120, 0),
        "\u{E03A}" => key_event("F10", "F10", 121, 0),
        "\u{E03B}" => key_event("F11", "F11", 122, 0),
        "\u{E03C}" => key_event("F12", "F12", 123, 0),
        "\u{E03D}" => key_event("Meta", "MetaLeft", 91, 1),
        "\u{E040}" => key_event("ZenkakuHankaku", "", 244, 0),
        "\u{E050}" => key_event("Shift", "ShiftRight", 16, 2),
        "\u{E051}" => key_event("Control", "ControlRight", 17, 2),
        "\u{E052}" => key_event("Alt", "AltRight", 18, 2),
        "\u{E053}" => key_event("Meta", "MetaRight", 91, 2),
        "\u{E054}" => key_event("PageUp", "Numpad9", 33, 3),
        "\u{E055}" => key_event("PageDown", "Numpad3", 34, 3),
        "\u{E056}" => key_event("End", "Numpad1", 35, 3),
        "\u{E057}" => key_event("Home", "Numpad7", 36, 3),
        "\u{E058}" => key_event("ArrowLeft", "Numpad4", 37, 3),
        "\u{E059}" => key_event("ArrowUp", "Numpad8", 38, 3),
        "\u{E05A}" => key_event("ArrowRight", "Numpad6", 39, 3),
        "\u{E05B}" => key_event("ArrowDown", "Numpad2", 40, 3),
        "\u{E05C}" => key_event("Insert", "Numpad0", 45, 3),
        "\u{E05D}" => key_event("Delete", "NumpadDecimal", 46, 3),
        _ => return None,
    })
}

/// Platform-agnostic trait for `WebView` operations.
#[async_trait]
#[allow(clippy::too_many_lines)]
pub trait PlatformExecutor<R: Runtime>: Send + Sync {
    fn window(&self) -> &WebviewWindow<R>;

    fn script_timeout_ms(&self) -> Option<u64>;

    async fn evaluate_js(&self, script: &str) -> Result<Value, WebDriverErrorResponse>;

    async fn navigate(&self, url: &str) -> Result<(), WebDriverErrorResponse> {
        let script = format!(
            r"window.location.href = '{}'; null;",
            url.replace('\\', "\\\\").replace('\'', "\\'")
        );
        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn get_url(&self) -> Result<String, WebDriverErrorResponse> {
        let result = self.evaluate_js("window.location.href").await?;
        extract_string_value(&result)
    }

    async fn get_title(&self) -> Result<String, WebDriverErrorResponse> {
        let result = self.evaluate_js("document.title").await?;
        extract_string_value(&result)
    }

    async fn go_back(&self) -> Result<(), WebDriverErrorResponse> {
        self.evaluate_js("window.history.back(); null;").await?;
        Ok(())
    }

    async fn go_forward(&self) -> Result<(), WebDriverErrorResponse> {
        self.evaluate_js("window.history.forward(); null;").await?;
        Ok(())
    }

    async fn refresh(&self) -> Result<(), WebDriverErrorResponse> {
        self.evaluate_js("window.location.reload(); null;").await?;
        Ok(())
    }

    async fn get_source(&self) -> Result<String, WebDriverErrorResponse> {
        let result = self
            .evaluate_js("document.documentElement.outerHTML")
            .await?;
        extract_string_value(&result)
    }

    async fn find_element(
        &self,
        strategy_js: &str,
        js_var: &str,
    ) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r#"(function() {{
                var el = {strategy_js};
                if (el) {{
                    window.{js_var} = el;
                    return true;
                }}
                return false;
            }})()"#
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn find_elements(
        &self,
        strategy_js: &str,
        js_var_prefix: &str,
    ) -> Result<usize, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var elements = {strategy_js};
                var count = elements.length;
                for (var i = 0; i < count; i++) {{
                    window['{js_var_prefix}' + i] = elements[i];
                }}
                return count;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_usize_value(&result)
    }

    async fn find_element_from_element(
        &self,
        parent_js_var: &str,
        strategy_js: &str,
        js_var: &str,
    ) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var parent = window.{parent_js_var};
                if (!parent || !parent.isConnected) {{
                    throw new Error('stale element reference');
                }}
                var el = {strategy_js};
                if (el) {{
                    window.{js_var} = el;
                    return true;
                }}
                return false;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn find_elements_from_element(
        &self,
        parent_js_var: &str,
        strategy_js: &str,
        js_var_prefix: &str,
    ) -> Result<usize, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var parent = window.{parent_js_var};
                if (!parent || !parent.isConnected) {{
                    throw new Error('stale element reference');
                }}
                var elements = {strategy_js};
                var count = elements.length;
                for (var i = 0; i < count; i++) {{
                    window['{js_var_prefix}' + i] = elements[i];
                }}
                return count;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_usize_value(&result)
    }

    async fn get_element_text(&self, js_var: &str) -> Result<String, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                return el.textContent || '';
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_string_value(&result)
    }

    async fn get_element_tag_name(&self, js_var: &str) -> Result<String, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                return el.tagName.toLowerCase();
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_string_value(&result)
    }

    /// Get element attribute value
    /// Per W3C `WebDriver` spec, certain attributes should return current property values:
    /// - "value" on input/textarea returns current value property
    /// - "checked" on checkbox/radio returns current checked state
    /// - "selected" on option returns current selected state
    async fn get_element_attribute(
        &self,
        js_var: &str,
        name: &str,
    ) -> Result<Option<String>, WebDriverErrorResponse> {
        let escaped_name = name.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                var attrName = '{escaped_name}'.toLowerCase();
                var tagName = el.tagName.toLowerCase();

                // Per W3C WebDriver spec, return property values for certain attributes
                if (attrName === 'value') {{
                    if (tagName === 'input' || tagName === 'textarea') {{
                        return el.value;
                    }}
                }}
                if (attrName === 'checked') {{
                    if (tagName === 'input' && (el.type === 'checkbox' || el.type === 'radio')) {{
                        return el.checked ? 'true' : null;
                    }}
                }}
                if (attrName === 'selected') {{
                    if (tagName === 'option') {{
                        return el.selected ? 'true' : null;
                    }}
                }}

                return el.getAttribute('{escaped_name}');
            }})()"
        );
        let result = self.evaluate_js(&script).await?;

        if let Some(value) = result.get("value") {
            if value.is_null() {
                return Ok(None);
            }
            if let Some(s) = value.as_str() {
                return Ok(Some(s.to_string()));
            }
        }
        Ok(None)
    }

    async fn get_element_property(
        &self,
        js_var: &str,
        name: &str,
    ) -> Result<Value, WebDriverErrorResponse> {
        let escaped_name = name.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                return el['{escaped_name}'];
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_value(&result)
    }

    async fn get_element_css_value(
        &self,
        js_var: &str,
        property: &str,
    ) -> Result<String, WebDriverErrorResponse> {
        let escaped_prop = property.replace('\\', "\\\\").replace('\'', "\\'");
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                return window.getComputedStyle(el).getPropertyValue('{escaped_prop}');
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_string_value(&result)
    }

    async fn get_element_rect(&self, js_var: &str) -> Result<ElementRect, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                var rect = el.getBoundingClientRect();
                return {{
                    x: rect.x + window.scrollX,
                    y: rect.y + window.scrollY,
                    width: rect.width,
                    height: rect.height
                }};
            }})()"
        );
        let result = self.evaluate_js(&script).await?;

        if let Some(value) = result.get("value") {
            return Ok(ElementRect {
                x: value.get("x").and_then(Value::as_f64).unwrap_or(0.0),
                y: value.get("y").and_then(Value::as_f64).unwrap_or(0.0),
                width: value.get("width").and_then(Value::as_f64).unwrap_or(0.0),
                height: value.get("height").and_then(Value::as_f64).unwrap_or(0.0),
            });
        }
        Ok(ElementRect::default())
    }

    /// Get an element's in-view center point in **client (viewport)** coordinates,
    /// scrolling it into view first. Unlike [`Executor::get_element_rect`], this
    /// does not add scroll offsets: pointer events dispatch against viewport
    /// coordinates (`clientX`/`clientY`), so the center must be viewport-relative.
    async fn get_element_center(&self, js_var: &str) -> Result<(i32, i32), WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                el.scrollIntoView({{ behavior: 'instant', block: 'center', inline: 'center' }});
                var r = el.getBoundingClientRect();
                return {{
                    x: Math.floor(r.left + r.width / 2),
                    y: Math.floor(r.top + r.height / 2)
                }};
            }})()"
        );
        let result = self.evaluate_js(&script).await?;

        let value = result.get("value").cloned().ok_or_else(|| {
            WebDriverErrorResponse::unknown_error("element center script returned no value")
        })?;

        #[derive(serde::Deserialize)]
        struct Center {
            x: i32,
            y: i32,
        }
        let center: Center = serde_json::from_value(value).map_err(|err| {
            WebDriverErrorResponse::unknown_error(&format!("could not read element center: {err}"))
        })?;
        Ok((center.x, center.y))
    }

    async fn is_element_displayed(&self, js_var: &str) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                var style = window.getComputedStyle(el);
                return style.display !== 'none' && style.visibility !== 'hidden' && el.offsetParent !== null;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn is_element_enabled(&self, js_var: &str) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                return !el.matches(':disabled');
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn is_element_keyboard_interactable(
        &self,
        js_var: &str,
    ) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                if (el === document.body || el === document.documentElement) return true;
                if (el.matches(':disabled') || el.getClientRects().length === 0) return false;
                if (el.isContentEditable || el.tabIndex >= 0) return true;
                return /^(INPUT|TEXTAREA|SELECT|BUTTON|A|IFRAME)$/.test(el.tagName);
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn is_element_selected(&self, js_var: &str) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                if (el.tagName === 'INPUT' && (el.type === 'checkbox' || el.type === 'radio')) {{
                    return el.checked;
                }}
                if (el.tagName === 'OPTION') {{
                    return el.selected;
                }}
                return false;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn click_element(&self, js_var: &str) -> Result<(), WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                if (el.tagName === 'INPUT' && el.type === 'file') {{
                    throw new Error('__tauri_wd_error__:invalid argument: file input elements cannot be clicked');
                }}

                el.scrollIntoView({{ block: 'center', inline: 'center' }});
                var target = el.tagName === 'OPTION' ? el.closest('select') : el;
                if (window.getComputedStyle(target).pointerEvents === 'none') {{
                    throw new Error('__tauri_wd_error__:element not interactable: element does not receive pointer events');
                }}
                var rects = Array.from(target.getClientRects()).filter(function(rect) {{
                    return rect.width > 0 && rect.height > 0 &&
                        rect.bottom > 0 && rect.right > 0 &&
                        rect.top < window.innerHeight && rect.left < window.innerWidth;
                }});
                if (rects.length === 0) {{
                    throw new Error('__tauri_wd_error__:element not interactable: element has no in-view center point');
                }}

                var rect = rects[0];
                var left = Math.max(0, Math.min(rect.left, rect.right));
                var right = Math.min(window.innerWidth, Math.max(rect.left, rect.right));
                var top = Math.max(0, Math.min(rect.top, rect.bottom));
                var bottom = Math.min(window.innerHeight, Math.max(rect.top, rect.bottom));
                var x = Math.floor((left + right) / 2);
                var y = Math.floor((top + bottom) / 2);
                var hit = document.elementFromPoint(x, y);
                if (!hit || (hit !== target && !target.contains(hit))) {{
                    if (!hit || (hit && hit.contains(target))) {{
                        throw new Error('__tauri_wd_error__:element not interactable: center point is outside the pointer-interactable paint tree');
                    }}
                    var description = hit ? hit.tagName.toLowerCase() : 'nothing';
                    throw new Error(
                        '__tauri_wd_error__:element click intercepted: center point is obscured by ' + description
                    );
                }}

                if (el.matches(':disabled') || target.matches(':disabled')) {{
                    return true;
                }}

                var eventTarget = hit;
                function pointer(type, bubbles, buttons) {{
                    return eventTarget.dispatchEvent(new PointerEvent(type, {{
                        bubbles: bubbles,
                        cancelable: true,
                        composed: true,
                        clientX: x,
                        clientY: y,
                        button: 0,
                        buttons: buttons,
                        pointerId: 1,
                        pointerType: 'mouse',
                        isPrimary: true
                    }}));
                }}
                function mouse(type, bubbles, buttons) {{
                    return eventTarget.dispatchEvent(new MouseEvent(type, {{
                        bubbles: bubbles,
                        cancelable: true,
                        composed: true,
                        view: window,
                        detail: type === 'click' ? 1 : 0,
                        clientX: x,
                        clientY: y,
                        button: 0,
                        buttons: buttons
                    }}));
                }}

                pointer('pointerover', true, 0);
                mouse('mouseover', true, 0);
                pointer('pointerenter', false, 0);
                mouse('mouseenter', false, 0);
                pointer('pointermove', true, 0);
                mouse('mousemove', true, 0);
                var pointerDown = pointer('pointerdown', true, 1);
                var mouseDown = mouse('mousedown', true, 1);
                if (pointerDown && mouseDown && typeof target.focus === 'function') {{
                    target.focus({{ preventScroll: true }});
                }}
                pointer('pointerup', true, 0);
                mouse('mouseup', true, 0);

                if (el.tagName === 'OPTION') {{
                    if (!el.matches(':disabled') && !target.matches(':disabled')) {{
                        if (!target.multiple) {{
                            Array.from(target.options).forEach(function(option) {{
                                option.selected = option === el;
                            }});
                        }} else {{
                            el.selected = !el.selected;
                        }}
                        target.dispatchEvent(new Event('input', {{ bubbles: true }}));
                        target.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    }}
                }} else {{
                    mouse('click', true, 0);
                }}
                return true;
            }})()"
        );
        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn clear_element(&self, js_var: &str) -> Result<(), WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                el.focus();
                if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {{
                    var nativeInputValueSetter = Object.getOwnPropertyDescriptor(
                        el.tagName === 'INPUT' ? window.HTMLInputElement.prototype : window.HTMLTextAreaElement.prototype,
                        'value'
                    ).set;
                    nativeInputValueSetter.call(el, '');
                    var inputEvent = new InputEvent('input', {{
                        bubbles: true,
                        cancelable: true,
                        inputType: 'deleteContentBackward'
                    }});
                    el.dispatchEvent(inputEvent);
                    var changeEvent = new Event('change', {{ bubbles: true }});
                    el.dispatchEvent(changeEvent);
                }} else if (el.isContentEditable) {{
                    el.innerHTML = '';
                }}
                return true;
            }})()"
        );
        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn send_keys_to_element(
        &self,
        js_var: &str,
        text: &str,
    ) -> Result<(), WebDriverErrorResponse> {
        let text_json = serde_json::to_string(text)
            .map_err(|error| WebDriverErrorResponse::invalid_argument(&error.to_string()))?;
        let script = format!(
            r#"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                if (el.tagName === 'IFRAME' && el.contentDocument) {{
                    el.scrollIntoView({{ block: 'center', inline: 'center' }});
                    el.focus();
                    var childDocument = el.contentDocument;
                    var childTarget = childDocument.activeElement;
                    if (!childTarget || childTarget === childDocument.body ||
                        childTarget === childDocument.documentElement) {{
                        childTarget = childDocument.querySelector(
                            'input,textarea,select,button,a[href],[tabindex],[contenteditable]'
                        );
                    }}
                    if (childTarget) {{
                        childTarget.focus();
                        el = childTarget;
                    }}
                }}
                var documentTarget =
                    el === el.ownerDocument.body || el === el.ownerDocument.documentElement;
                if (el.matches(':disabled')) {{
                    throw new Error('__tauri_wd_error__:element not interactable: element is disabled');
                }}

                var input = el.tagName === 'INPUT';
                var textarea = el.tagName === 'TEXTAREA';
                var textInput = input && ![
                    'button', 'checkbox', 'color', 'file', 'hidden', 'image',
                    'radio', 'range', 'reset', 'submit'
                ].includes(el.type);
                var editable = (textInput || textarea || el.isContentEditable) && !el.readOnly;
                if (!documentTarget && el.getClientRects().length === 0) {{
                    throw new Error('__tauri_wd_error__:element not interactable: element has no rendered box');
                }}

                if (!documentTarget) {{
                    el.scrollIntoView({{ block: 'center', inline: 'center' }});
                }}
                var ownerDocument = el.ownerDocument;
                var wasActive = ownerDocument.activeElement === el;
                if (!wasActive && documentTarget) {{
                    var hadTabIndex = el.hasAttribute('tabindex');
                    var previousTabIndex = el.getAttribute('tabindex');
                    el.setAttribute('tabindex', '-1');
                    el.focus();
                    if (hadTabIndex) el.setAttribute('tabindex', previousTabIndex);
                    else el.removeAttribute('tabindex');
                }} else if (!wasActive) {{
                    el.focus();
                }}
                if (!documentTarget && ownerDocument.activeElement !== el) {{
                    throw new Error('__tauri_wd_error__:element not interactable: element cannot receive keyboard focus');
                }}
                if (!wasActive && (textInput || textarea)) {{
                    try {{
                        el.setSelectionRange(el.value.length, el.value.length);
                    }} catch (_) {{}}
                }}

                if (input && el.type === 'date' && !/[\uE000-\uF8FF]/.test({text_json})) {{
                    var dateText = {text_json};
                    var dateMatch = dateText.match(/^(\d{{2}})\/(\d{{2}})\/(\d{{4}})$/);
                    var dateValue = dateMatch
                        ? dateMatch[3] + '-' + dateMatch[1] + '-' + dateMatch[2]
                        : dateText;
                    var dateSetter = Object.getOwnPropertyDescriptor(
                        el.ownerDocument.defaultView.HTMLInputElement.prototype,
                        'value'
                    ).set;
                    dateSetter.call(el, dateValue);
                    el.dispatchEvent(new el.ownerDocument.defaultView.InputEvent('input', {{
                        bubbles: true,
                        composed: true,
                        inputType: 'insertText',
                        data: dateText
                    }}));
                    el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                    return true;
                }}

                var modifiers = {{
                    alt: false,
                    control: false,
                    meta: false,
                    shift: false
                }};
                var heldModifiers = [];
                var currentLocation = 0;
                var selectionAnchor = null;
                var special = {{
                    '\uE001': ['Cancel', '', 3, 'none'],
                    '\uE002': ['Help', 'Help', 6, 'none'],
                    '\uE003': ['Backspace', 'Backspace', 8, 'backspace'],
                    '\uE004': ['Tab', 'Tab', 9, 'tab'],
                    '\uE005': ['Clear', '', 12, 'clear'],
                    '\uE006': ['Enter', 'Enter', 13, 'enter'],
                    '\uE007': ['Enter', 'NumpadEnter', 13, 'enter'],
                    '\uE008': ['Shift', 'ShiftLeft', 16, 'shift'],
                    '\uE009': ['Control', 'ControlLeft', 17, 'control'],
                    '\uE00A': ['Alt', 'AltLeft', 18, 'alt'],
                    '\uE00B': ['Pause', 'Pause', 19, 'none'],
                    '\uE00C': ['Escape', 'Escape', 27, 'none'],
                    '\uE00D': [' ', 'Space', 32, 'space'],
                    '\uE00E': ['PageUp', 'PageUp', 33, 'none'],
                    '\uE00F': ['PageDown', 'PageDown', 34, 'none'],
                    '\uE010': ['End', 'End', 35, 'end'],
                    '\uE011': ['Home', 'Home', 36, 'home'],
                    '\uE012': ['ArrowLeft', 'ArrowLeft', 37, 'left'],
                    '\uE013': ['ArrowUp', 'ArrowUp', 38, 'none'],
                    '\uE014': ['ArrowRight', 'ArrowRight', 39, 'right'],
                    '\uE015': ['ArrowDown', 'ArrowDown', 40, 'none'],
                    '\uE016': ['Insert', 'Insert', 45, 'none'],
                    '\uE017': ['Delete', 'Delete', 46, 'delete'],
                    '\uE018': [';', '', 186, 'printable'],
                    '\uE019': ['=', 'NumpadEqual', 187, 'printable'],
                    '\uE01A': ['0', 'Numpad0', 96, 'printable'],
                    '\uE01B': ['1', 'Numpad1', 97, 'printable'],
                    '\uE01C': ['2', 'Numpad2', 98, 'printable'],
                    '\uE01D': ['3', 'Numpad3', 99, 'printable'],
                    '\uE01E': ['4', 'Numpad4', 100, 'printable'],
                    '\uE01F': ['5', 'Numpad5', 101, 'printable'],
                    '\uE020': ['6', 'Numpad6', 102, 'printable'],
                    '\uE021': ['7', 'Numpad7', 103, 'printable'],
                    '\uE022': ['8', 'Numpad8', 104, 'printable'],
                    '\uE023': ['9', 'Numpad9', 105, 'printable'],
                    '\uE024': ['*', 'NumpadMultiply', 106, 'printable'],
                    '\uE025': ['+', 'NumpadAdd', 107, 'printable'],
                    '\uE026': [',', 'NumpadComma', 108, 'printable'],
                    '\uE027': ['-', 'NumpadSubtract', 109, 'printable'],
                    '\uE028': ['.', 'NumpadDecimal', 110, 'printable'],
                    '\uE029': ['/', 'NumpadDivide', 111, 'printable'],
                    '\uE031': ['F1', 'F1', 112, 'none'],
                    '\uE032': ['F2', 'F2', 113, 'none'],
                    '\uE033': ['F3', 'F3', 114, 'none'],
                    '\uE034': ['F4', 'F4', 115, 'none'],
                    '\uE035': ['F5', 'F5', 116, 'none'],
                    '\uE036': ['F6', 'F6', 117, 'none'],
                    '\uE037': ['F7', 'F7', 118, 'none'],
                    '\uE038': ['F8', 'F8', 119, 'none'],
                    '\uE039': ['F9', 'F9', 120, 'none'],
                    '\uE03A': ['F10', 'F10', 121, 'none'],
                    '\uE03B': ['F11', 'F11', 122, 'none'],
                    '\uE03C': ['F12', 'F12', 123, 'none'],
                    '\uE03D': ['Meta', 'MetaLeft', 91, 'meta'],
                    '\uE040': ['ZenkakuHankaku', '', 244, 'none'],
                    '\uE050': ['Shift', 'ShiftRight', 16, 'shift'],
                    '\uE051': ['Control', 'ControlRight', 17, 'control'],
                    '\uE052': ['Alt', 'AltRight', 18, 'alt'],
                    '\uE053': ['Meta', 'MetaRight', 91, 'meta'],
                    '\uE054': ['PageUp', 'Numpad9', 33, 'none'],
                    '\uE055': ['PageDown', 'Numpad3', 34, 'none'],
                    '\uE056': ['End', 'Numpad1', 35, 'end'],
                    '\uE057': ['Home', 'Numpad7', 36, 'home'],
                    '\uE058': ['ArrowLeft', 'Numpad4', 37, 'left'],
                    '\uE059': ['ArrowUp', 'Numpad8', 38, 'none'],
                    '\uE05A': ['ArrowRight', 'Numpad6', 39, 'right'],
                    '\uE05B': ['ArrowDown', 'Numpad2', 40, 'none'],
                    '\uE05C': ['Insert', 'Numpad0', 45, 'none'],
                    '\uE05D': ['Delete', 'NumpadDecimal', 46, 'delete']
                }};
                var keyLocations = {{
                    '\uE007': 1,
                    '\uE008': 1, '\uE009': 1, '\uE00A': 1, '\uE03D': 1,
                    '\uE050': 2, '\uE051': 2, '\uE052': 2, '\uE053': 2,
                    '\uE019': 3, '\uE01A': 3, '\uE01B': 3, '\uE01C': 3,
                    '\uE01D': 3, '\uE01E': 3, '\uE01F': 3, '\uE020': 3,
                    '\uE021': 3, '\uE022': 3, '\uE023': 3, '\uE024': 3,
                    '\uE025': 3, '\uE026': 3, '\uE027': 3, '\uE028': 3,
                    '\uE029': 3, '\uE054': 3, '\uE055': 3, '\uE056': 3,
                    '\uE057': 3, '\uE058': 3, '\uE059': 3, '\uE05A': 3,
                    '\uE05B': 3, '\uE05C': 3, '\uE05D': 3
                }};
                var shifted = {{
                    '`': '~', '1': '!', '2': '@', '3': '#', '4': '$',
                    '5': '%', '6': '^', '7': '&', '8': '*', '9': '(',
                    '0': ')', '-': '_', '=': '+', '[': '{{', ']': '}}',
                    '\\': '|', ';': ':', "'": '"', ',': '<', '.': '>', '/': '?'
                }};

                function keyCodeFor(key) {{
                    if (key.length === 1) return key.toUpperCase().charCodeAt(0);
                    return 0;
                }}
                function codeFor(key) {{
                    if (/^[a-z]$/i.test(key)) return 'Key' + key.toUpperCase();
                    if (/^[0-9]$/.test(key)) return 'Digit' + key;
                    return {{
                        '`': 'Backquote', '\\': 'Backslash', '[': 'BracketLeft',
                        ']': 'BracketRight', ',': 'Comma', '=': 'Equal',
                        '-': 'Minus', '.': 'Period', "'": 'Quote',
                        ';': 'Semicolon', '/': 'Slash', ' ': 'Space'
                    }}[key] || key;
                }}
                function keyboard(type, key, code, keyCode) {{
                    return el.dispatchEvent(new el.ownerDocument.defaultView.KeyboardEvent(type, {{
                        bubbles: true,
                        cancelable: true,
                        composed: true,
                        key: key,
                        code: code,
                        keyCode: keyCode,
                        which: keyCode,
                        altKey: modifiers.alt,
                        ctrlKey: modifiers.control,
                        metaKey: modifiers.meta,
                        shiftKey: modifiers.shift,
                        location: currentLocation
                    }}));
                }}
                function beforeInput(inputType, data) {{
                    return el.dispatchEvent(new el.ownerDocument.defaultView.InputEvent('beforeinput', {{
                        bubbles: true,
                        cancelable: true,
                        composed: true,
                        inputType: inputType,
                        data: data
                    }}));
                }}
                function inputEvent(inputType, data) {{
                    el.dispatchEvent(new el.ownerDocument.defaultView.InputEvent('input', {{
                        bubbles: true,
                        composed: true,
                        inputType: inputType,
                        data: data
                    }}));
                }}
                function setTextControlValue(value, start, inputType, data) {{
                    var prototype = input
                        ? el.ownerDocument.defaultView.HTMLInputElement.prototype
                        : el.ownerDocument.defaultView.HTMLTextAreaElement.prototype;
                    var setter = Object.getOwnPropertyDescriptor(prototype, 'value').set;
                    setter.call(el, value);
                    try {{
                        el.setSelectionRange(start, start);
                    }} catch (_) {{}}
                    inputEvent(inputType, data);
                }}
                function insertText(data) {{
                    if (!editable || modifiers.control || modifiers.meta || modifiers.alt) return;
                    if (!beforeInput('insertText', data)) return;
                    if (textInput || textarea) {{
                        var start = el.selectionStart == null ? el.value.length : el.selectionStart;
                        var end = el.selectionEnd == null ? el.value.length : el.selectionEnd;
                        setTextControlValue(
                            el.value.slice(0, start) + data + el.value.slice(end),
                            start + data.length,
                            'insertText',
                            data
                        );
                    }} else {{
                        el.ownerDocument.execCommand('insertText', false, data);
                    }}
                }}
                function deleteText(backward) {{
                    if (!editable) return;
                    var inputType = backward ? 'deleteContentBackward' : 'deleteContentForward';
                    if (!beforeInput(inputType, null)) return;
                    if (textInput || textarea) {{
                        var start = el.selectionStart == null ? el.value.length : el.selectionStart;
                        var end = el.selectionEnd == null ? el.value.length : el.selectionEnd;
                        if (start === end) {{
                            if (backward && start > 0) start--;
                            if (!backward && end < el.value.length) end++;
                        }}
                        if (start !== end) {{
                            setTextControlValue(
                                el.value.slice(0, start) + el.value.slice(end),
                                start,
                                inputType,
                                null
                            );
                        }}
                    }} else {{
                        el.ownerDocument.execCommand(
                            backward ? 'delete' : 'forwardDelete',
                            false,
                            null
                        );
                    }}
                }}
                function moveCaret(action) {{
                    if (!(textInput || textarea) || el.selectionStart == null) return;
                    var start = el.selectionStart;
                    var end = el.selectionEnd;
                    if (modifiers.shift) {{
                        if (selectionAnchor === null) {{
                            selectionAnchor = el.selectionDirection === 'backward' ? end : start;
                            if (start === end) selectionAnchor = start;
                        }}
                        var focus = el.selectionDirection === 'backward' ? start : end;
                        if (action === 'left') focus = Math.max(0, focus - 1);
                        if (action === 'right') focus = Math.min(el.value.length, focus + 1);
                        if (action === 'home') focus = 0;
                        if (action === 'end') focus = el.value.length;
                        el.setSelectionRange(
                            Math.min(selectionAnchor, focus),
                            Math.max(selectionAnchor, focus),
                            focus < selectionAnchor ? 'backward' : 'forward'
                        );
                    }} else {{
                        selectionAnchor = null;
                        var position = end;
                        if (action === 'left') position =
                            start === end ? Math.max(0, start - 1) : start;
                        if (action === 'right') position = start === end
                            ? Math.min(el.value.length, end + 1)
                            : end;
                        if (action === 'home') position = 0;
                        if (action === 'end') position = el.value.length;
                        el.setSelectionRange(position, position);
                    }}
                }}
                function moveFocus() {{
                    var candidates = Array.from(el.ownerDocument.querySelectorAll(
                        'a[href],button,input,select,textarea,[tabindex]'
                    )).filter(function(node) {{
                        return !node.disabled && node.tabIndex >= 0 && node.getClientRects().length;
                    }});
                    var index = candidates.indexOf(el);
                    var offset = modifiers.shift ? -1 : 1;
                    var next = candidates[(index + offset + candidates.length) % candidates.length];
                    if (next) {{
                        next.focus();
                        if (el.ownerDocument.activeElement === next) {{
                            el = next;
                            input = el.tagName === 'INPUT';
                            textarea = el.tagName === 'TEXTAREA';
                            textInput = input && ![
                                'button', 'checkbox', 'color', 'file', 'hidden', 'image',
                                'radio', 'range', 'reset', 'submit'
                            ].includes(el.type);
                            editable =
                                (textInput || textarea || el.isContentEditable) && !el.readOnly;
                            selectionAnchor = null;
                        }}
                    }}
                }}
                function releaseModifiers() {{
                    for (var index = heldModifiers.length - 1; index >= 0; index--) {{
                        var held = heldModifiers[index];
                        modifiers[held[0][3]] = false;
                        currentLocation = held[1];
                        keyboard('keyup', held[0][0], held[0][1], held[0][2]);
                    }}
                    heldModifiers = [];
                    selectionAnchor = null;
                }}

                var keys = Array.from({text_json});
                for (var index = 0; index < keys.length; index++) {{
                    var raw = keys[index];
                    if (raw === '\uE000') {{
                        releaseModifiers();
                        continue;
                    }}
                    var spec = special[raw];
                    if (!spec && raw.codePointAt(0) >= 0xE000 && raw.codePointAt(0) <= 0xF8FF) {{
                        throw new Error('__tauri_wd_error__:invalid argument: unsupported WebDriver key');
                    }}

                    var key = spec ? spec[0] : raw;
                    var code = spec ? spec[1] : codeFor(key);
                    var keyCode = spec ? spec[2] : keyCodeFor(key);
                    var action = spec ? spec[3] : 'printable';
                    currentLocation = keyLocations[raw] || 0;

                    if (['alt', 'control', 'meta', 'shift'].includes(action)) {{
                        if (!modifiers[action]) {{
                            modifiers[action] = true;
                            keyboard('keydown', key, code, keyCode);
                            heldModifiers.push([spec, currentLocation]);
                        }}
                        continue;
                    }}

                    if (action === 'printable' || action === 'space') {{
                        if (modifiers.shift) {{
                            key = shifted[key] || key.toUpperCase();
                        }}
                    }}
                    var proceed = keyboard('keydown', key, code, keyCode);
                    if (proceed && (action === 'printable' || action === 'space')) {{
                        keyboard('keypress', key, code, keyCode);
                        insertText(key);
                    }} else if (proceed && action === 'backspace') {{
                        deleteText(true);
                    }} else if (proceed && action === 'delete') {{
                        deleteText(false);
                    }} else if (proceed && ['left', 'right', 'home', 'end'].includes(action)) {{
                        moveCaret(action);
                    }} else if (proceed && action === 'tab') {{
                        moveFocus();
                    }} else if (proceed && action === 'clear' && (textInput || textarea)) {{
                        el.select();
                        deleteText(true);
                    }} else if (proceed && action === 'enter') {{
                        keyboard('keypress', key, code, keyCode);
                        if (textarea || el.isContentEditable) {{
                            insertText('\n');
                        }} else if (el.form && typeof el.form.requestSubmit === 'function') {{
                            el.form.requestSubmit();
                        }}
                    }}
                    keyboard('keyup', key, code, keyCode);
                }}
                releaseModifiers();
                return true;
            }})()"#
        );
        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn is_file_input(&self, js_var: &str) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                return el.tagName === 'INPUT' && el.type === 'file';
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn set_file_input_files(
        &self,
        js_var: &str,
        files: &[(String, String)],
    ) -> Result<(), WebDriverErrorResponse> {
        let files_json = serde_json::to_string(files)
            .map_err(|error| WebDriverErrorResponse::unknown_error(&error.to_string()))?;
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                if (el.tagName !== 'INPUT' || el.type !== 'file') {{
                    throw new Error('element is not a file input');
                }}
                var specs = {files_json};
                if (!el.multiple && specs.length > 1) {{
                    throw new Error('multiple files were provided to a single-file input');
                }}
                var transfer = new DataTransfer();
                for (var spec of specs) {{
                    var binary = atob(spec[1]);
                    var bytes = new Uint8Array(binary.length);
                    for (var index = 0; index < binary.length; index++) {{
                        bytes[index] = binary.charCodeAt(index);
                    }}
                    transfer.items.add(new File([bytes], spec[0]));
                }}
                el.files = transfer.files;
                el.dispatchEvent(new Event('input', {{ bubbles: true }}));
                el.dispatchEvent(new Event('change', {{ bubbles: true }}));
                return true;
            }})()"
        );
        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn get_active_element(&self, js_var: &str) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = document.activeElement;
                if (el && el !== document.body) {{
                    window.{js_var} = el;
                    return true;
                }}
                return false;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn get_element_computed_role(
        &self,
        js_var: &str,
    ) -> Result<String, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}

                var explicitRole = el.getAttribute('role');
                if (explicitRole) return explicitRole;

                // Try computedRole if available (Chrome/Edge)
                if (el.computedRole) return el.computedRole;

                // Compute implicit role based on element type
                var tag = el.tagName.toLowerCase();
                var type = el.type ? el.type.toLowerCase() : '';

                var roleMap = {{
                    'a': el.hasAttribute('href') ? 'link' : 'generic',
                    'article': 'article',
                    'aside': 'complementary',
                    'button': 'button',
                    'datalist': 'listbox',
                    'details': 'group',
                    'dialog': 'dialog',
                    'fieldset': 'group',
                    'figure': 'figure',
                    'footer': 'contentinfo',
                    'form': 'form',
                    'h1': 'heading',
                    'h2': 'heading',
                    'h3': 'heading',
                    'h4': 'heading',
                    'h5': 'heading',
                    'h6': 'heading',
                    'header': 'banner',
                    'hr': 'separator',
                    'img': el.getAttribute('alt') === '' ? 'presentation' : 'img',
                    'li': 'listitem',
                    'main': 'main',
                    'menu': 'list',
                    'meter': 'meter',
                    'nav': 'navigation',
                    'ol': 'list',
                    'optgroup': 'group',
                    'option': 'option',
                    'output': 'status',
                    'progress': 'progressbar',
                    'section': 'region',
                    'select': el.multiple ? 'listbox' : 'combobox',
                    'summary': 'button',
                    'table': 'table',
                    'tbody': 'rowgroup',
                    'td': 'cell',
                    'textarea': 'textbox',
                    'tfoot': 'rowgroup',
                    'th': 'columnheader',
                    'thead': 'rowgroup',
                    'tr': 'row',
                    'ul': 'list'
                }};

                if (tag === 'input') {{
                    var inputRoles = {{
                        'button': 'button',
                        'checkbox': 'checkbox',
                        'email': 'textbox',
                        'image': 'button',
                        'number': 'spinbutton',
                        'radio': 'radio',
                        'range': 'slider',
                        'reset': 'button',
                        'search': 'searchbox',
                        'submit': 'button',
                        'tel': 'textbox',
                        'text': 'textbox',
                        'url': 'textbox'
                    }};
                    return inputRoles[type] || 'textbox';
                }}

                return roleMap[tag] || '';
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_string_value(&result)
    }

    async fn get_element_computed_label(
        &self,
        js_var: &str,
    ) -> Result<String, WebDriverErrorResponse> {
        let script = format!(
            r#"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}

                // Try computedName if available (Chrome/Edge)
                if (el.computedName) return el.computedName;

                var labelledBy = el.getAttribute('aria-labelledby');
                if (labelledBy) {{
                    var labels = labelledBy.split(/\s+/).map(function(id) {{
                        var labelEl = document.getElementById(id);
                        return labelEl ? labelEl.textContent : '';
                    }});
                    var combined = labels.join(' ').trim();
                    if (combined) return combined;
                }}

                var ariaLabel = el.getAttribute('aria-label');
                if (ariaLabel) return ariaLabel;

                var tag = el.tagName.toLowerCase();
                if (tag === 'input' || tag === 'textarea' || tag === 'select') {{
                    if (el.id) {{
                        var label = document.querySelector("label[for='" + el.id + "']");
                        if (label) return label.textContent.trim();
                    }}
                    var parentLabel = el.closest('label');
                    if (parentLabel) {{
                        var clone = parentLabel.cloneNode(true);
                        var inputs = clone.querySelectorAll('input, textarea, select');
                        inputs.forEach(function(input) {{ input.remove(); }});
                        var labelText = clone.textContent.trim();
                        if (labelText) return labelText;
                    }}
                    if (el.placeholder) return el.placeholder;
                }}

                if (tag === 'button' || tag === 'a') {{
                    return el.textContent.trim();
                }}

                if (tag === 'img') {{
                    return el.getAttribute('alt') || '';
                }}

                var title = el.getAttribute('title');
                if (title) return title;

                return el.textContent ? el.textContent.trim() : '';
            }})()"#
        );
        let result = self.evaluate_js(&script).await?;
        extract_string_value(&result)
    }

    async fn get_element_shadow_root(
        &self,
        js_var: &str,
        shadow_var: &str,
    ) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                var shadow = el.shadowRoot;
                if (shadow) {{
                    window.{shadow_var} = shadow;
                    return true;
                }}
                return false;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn find_element_from_shadow(
        &self,
        shadow_var: &str,
        strategy_js: &str,
        js_var: &str,
    ) -> Result<bool, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var shadow = window.{shadow_var};
                if (!shadow) {{
                    throw new Error('no such shadow root');
                }}
                var el = {strategy_js};
                if (el) {{
                    window.{js_var} = el;
                    return true;
                }}
                return false;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_bool_value(&result)
    }

    async fn find_elements_from_shadow(
        &self,
        shadow_var: &str,
        strategy_js: &str,
        js_var_prefix: &str,
    ) -> Result<usize, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var shadow = window.{shadow_var};
                if (!shadow) {{
                    throw new Error('no such shadow root');
                }}
                var elements = {strategy_js};
                var count = elements.length;
                for (var i = 0; i < count; i++) {{
                    window['{js_var_prefix}' + i] = elements[i];
                }}
                return count;
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        extract_usize_value(&result)
    }

    async fn execute_script(
        &self,
        script: &str,
        args: &[Value],
    ) -> Result<Value, WebDriverErrorResponse> {
        let args_json = serde_json::to_string(args)
            .map_err(|e| WebDriverErrorResponse::invalid_argument(&e.to_string()))?;

        let result_var = format!("__tauri_wd_exec_{}", uuid::Uuid::new_v4());

        // Wrapper script that:
        // 1. Executes the user's script as a function body (per W3C WebDriver spec §13.2.2)
        // 2. Stores result in a global variable for polling
        // Note: We use an IIFE that returns `undefined` to avoid Promise serialization issues
        //
        // The script is treated as a function body. Clients that want to return a value must
        // include an explicit `return` statement. This supports function-object
        // wrapping and raw string scripts such as `"return document.title"`.
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

                function serializeValue(value, seen) {{
                    if (value === null || value === undefined) return null;
                    if (typeof value === 'boolean') return value;
                    if (typeof value === 'number') {{
                        if (!isFinite(value)) return null;
                        return value;
                    }}
                    if (typeof value === 'string') return value;
                    if (typeof value === 'function') return null;
                    if (typeof value === 'symbol') return null;
                    if (typeof value === 'bigint') throw new TypeError('BigInt is not JSON serializable');
                    if (value && value.nodeType === 1) return storeNode(value, ELEMENT_KEY);
                    if (value && value.nodeType === 11 && value.host) {{
                        return storeNode(value, SHADOW_KEY);
                    }}
                    if (typeof value === 'object') {{
                        if (value[ELEMENT_KEY]) return value;
                        if (value[SHADOW_KEY]) return value;
                        seen = seen || new WeakSet();
                        if (seen.has(value)) throw new TypeError('cyclic object value');
                        seen.add(value);
                        if (Array.isArray(value) ||
                            value instanceof NodeList ||
                            value instanceof HTMLCollection) {{
                            var list = Array.from(value).map(function(item) {{
                                return serializeValue(item, seen);
                            }});
                            seen.delete(value);
                            return list;
                        }}
                        var result = {{}};
                        try {{
                            for (var key in value) {{
                                if (Object.prototype.hasOwnProperty.call(value, key)) {{
                                    result[key] = serializeValue(value[key], seen);
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

                (async function() {{
                    try {{
                        var args = {args_json}.map(deserializeArg);
                        var raw_result = await (async function() {{ {script} }}).apply(null, args);
                        var serialized = serializeValue(raw_result);
                        window['{result_var}'] = {{ __wd_success: true, __wd_value: serialized }};
                    }} catch (e) {{
                        window['{result_var}'] = {{ __wd_success: false, __wd_error: e.message || String(e) }};
                    }}
                }})();

                // Return undefined to avoid Promise serialization issues
                return undefined;
            }})()",
        );

        self.evaluate_js(&wrapper).await?;

        let poll_script = format!("window['{}']", result_var);
        let timeout = self.script_timeout_ms().map(Duration::from_millis);
        let start = std::time::Instant::now();

        loop {
            let poll_result = self.evaluate_js(&poll_script).await?;
            let inner = poll_result.get("value").cloned().unwrap_or(Value::Null);

            if !inner.is_null() && inner.get("__wd_success").is_some() {
                let cleanup_script = format!("delete window['{}']", result_var);
                let _ = self.evaluate_js(&cleanup_script).await;

                return extract_script_result_from_inner(&inner);
            }

            if timeout.is_some_and(|timeout| start.elapsed() >= timeout) {
                let cleanup_script = format!("delete window['{}']", result_var);
                let _ = self.evaluate_js(&cleanup_script).await;

                return Err(WebDriverErrorResponse::script_timeout());
            }

            tokio::time::sleep(IMPLICIT_POLL_INTERVAL).await;
        }
    }

    /// Execute asynchronous JavaScript with callback.
    ///
    /// Each platform must implement this using native message handlers.
    async fn execute_async_script(
        &self,
        script: &str,
        args: &[Value],
    ) -> Result<Value, WebDriverErrorResponse>;

    async fn take_screenshot(&self) -> Result<String, WebDriverErrorResponse>;

    async fn take_element_screenshot(
        &self,
        js_var: &str,
    ) -> Result<String, WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = window.{js_var};
                if (!el || !el.isConnected) {{
                    throw new Error('stale element reference');
                }}
                el.scrollIntoView({{ block: 'center', inline: 'center' }});
                var rect = el.getBoundingClientRect();
                var left = Math.max(0, rect.left);
                var top = Math.max(0, rect.top);
                var right = Math.min(window.innerWidth, rect.right);
                var bottom = Math.min(window.innerHeight, rect.bottom);
                if (right <= left || bottom <= top) {{
                    throw new Error('__tauri_wd_error__:unable to capture screen: element has no visible region');
                }}
                var offsetX = 0;
                var offsetY = 0;
                var context = window;
                while (context !== context.top) {{
                    var frame = context.frameElement;
                    if (!frame) {{
                        throw new Error('__tauri_wd_error__:unable to capture screen: frame position is unavailable');
                    }}
                    var frameRect = frame.getBoundingClientRect();
                    offsetX += frameRect.left + frame.clientLeft;
                    offsetY += frameRect.top + frame.clientTop;
                    context = context.parent;
                }}
                return {{
                    x: left + offsetX,
                    y: top + offsetY,
                    width: right - left,
                    height: bottom - top,
                    viewportWidth: context.innerWidth,
                    viewportHeight: context.innerHeight
                }};
            }})()"
        );
        let result = self.evaluate_js(&script).await?;
        let value = extract_value(&result)?;
        let rect: ScreenshotRect = serde_json::from_value(value).map_err(|error| {
            WebDriverErrorResponse::unable_to_capture_screen(&format!(
                "Invalid element screenshot rectangle: {error}"
            ))
        })?;
        let screenshot = self
            .take_screenshot()
            .await
            .map_err(|error| WebDriverErrorResponse::unable_to_capture_screen(&error.message))?;
        crop_screenshot(&screenshot, &rect)
    }

    async fn dispatch_key_event(
        &self,
        key: &str,
        is_down: bool,
        modifiers: &ModifierState,
    ) -> Result<(), WebDriverErrorResponse> {
        let Some(event) = webdriver_key_event(key) else {
            let ch = key.chars().next().unwrap_or(' ');
            let upper = ch.to_ascii_uppercase();
            let code = if ch.is_ascii_alphabetic() {
                format!("Key{upper}")
            } else if ch.is_ascii_digit() {
                format!("Digit{ch}")
            } else {
                key.to_string()
            };
            return self
                .dispatch_regular_key(key, &code, is_down, modifiers)
                .await;
        };
        let WebDriverKeyEvent {
            key: js_key,
            code: js_code,
            key_code,
            location,
        } = event;
        let ctrl_key = modifiers.ctrl;
        let meta_key = modifiers.meta;
        let shift_key = modifiers.shift;
        let alt_key = modifiers.alt;

        let event_type = if is_down { "keydown" } else { "keyup" };

        let script = if is_down && (js_key == "Backspace" || js_key == "Delete") {
            format!(
                r"(function() {{
                    var activeEl = document.activeElement || document.body;

                    var keydownEvent = new KeyboardEvent('keydown', {{
                        key: '{js_key}',
                        code: '{js_code}',
                        keyCode: {key_code},
                        which: {key_code},
                        location: {location},
                        ctrlKey: {ctrl_key},
                        metaKey: {meta_key},
                        shiftKey: {shift_key},
                        altKey: {alt_key},
                        bubbles: true,
                        cancelable: true
                    }});
                    activeEl.dispatchEvent(keydownEvent);

                    if (activeEl.tagName === 'INPUT' || activeEl.tagName === 'TEXTAREA') {{
                        var nativeInputValueSetter = Object.getOwnPropertyDescriptor(
                            activeEl.tagName === 'INPUT'
                                ? window.HTMLInputElement.prototype
                                : window.HTMLTextAreaElement.prototype,
                            'value'
                        ).set;

                        var currentValue = activeEl.value;
                        var selStart = activeEl.selectionStart;
                        var selEnd = activeEl.selectionEnd;
                        var newValue;
                        var inputType;

                        if (selStart !== selEnd) {{
                            newValue = currentValue.slice(0, selStart) + currentValue.slice(selEnd);
                            inputType = 'deleteContentBackward';
                            nativeInputValueSetter.call(activeEl, newValue);
                            activeEl.setSelectionRange(selStart, selStart);
                        }} else if ('{js_key}' === 'Backspace' && selStart > 0) {{
                            newValue = currentValue.slice(0, selStart - 1) + currentValue.slice(selStart);
                            inputType = 'deleteContentBackward';
                            nativeInputValueSetter.call(activeEl, newValue);
                            activeEl.setSelectionRange(selStart - 1, selStart - 1);
                        }} else if ('{js_key}' === 'Delete' && selStart < currentValue.length) {{
                            newValue = currentValue.slice(0, selStart) + currentValue.slice(selStart + 1);
                            inputType = 'deleteContentForward';
                            nativeInputValueSetter.call(activeEl, newValue);
                            activeEl.setSelectionRange(selStart, selStart);
                        }} else {{
                            return true; // Nothing to delete
                        }}

                        var inputEvent = new InputEvent('input', {{
                            bubbles: true,
                            cancelable: true,
                            inputType: inputType
                        }});
                        activeEl.dispatchEvent(inputEvent);
                    }}

                    return true;
                }})()"
            )
        } else if is_down
            && (js_key == "ArrowDown"
                || js_key == "ArrowUp"
                || js_key == "ArrowLeft"
                || js_key == "ArrowRight")
        {
            let go_forward = js_key == "ArrowDown" || js_key == "ArrowRight";
            format!(
                r#"(function() {{
                    var activeEl = document.activeElement || document.body;

                    var keydownEvent = new KeyboardEvent('keydown', {{
                        key: '{js_key}',
                        code: '{js_code}',
                        keyCode: {key_code},
                        which: {key_code},
                        location: {location},
                        ctrlKey: {ctrl_key},
                        metaKey: {meta_key},
                        shiftKey: {shift_key},
                        altKey: {alt_key},
                        bubbles: true,
                        cancelable: true
                    }});
                    activeEl.dispatchEvent(keydownEvent);

                    if (activeEl.tagName === 'INPUT' && activeEl.type === 'radio' && activeEl.name) {{
                        var name = activeEl.name;
                        var radios = Array.from(document.querySelectorAll("input[type='radio'][name='" + name + "']"));
                        var currentIndex = radios.indexOf(activeEl);

                        if (currentIndex !== -1 && radios.length > 1) {{
                            var nextIndex;
                            if ({go_forward}) {{
                                nextIndex = (currentIndex + 1) % radios.length;
                            }} else {{
                                nextIndex = (currentIndex - 1 + radios.length) % radios.length;
                            }}

                            var nextRadio = radios[nextIndex];
                            nextRadio.checked = true;
                            nextRadio.focus();

                            var changeEvent = new Event('change', {{ bubbles: true }});
                            nextRadio.dispatchEvent(changeEvent);
                        }}
                    }}

                    return true;
                }})()"#
            )
        } else {
            format!(
                r"(function() {{
                    var event = new KeyboardEvent('{event_type}', {{
                        key: '{js_key}',
                        code: '{js_code}',
                        keyCode: {key_code},
                        which: {key_code},
                        location: {location},
                        ctrlKey: {ctrl_key},
                        metaKey: {meta_key},
                        shiftKey: {shift_key},
                        altKey: {alt_key},
                        bubbles: true,
                        cancelable: true
                    }});
                    var activeEl = document.activeElement || document.body;
                    activeEl.dispatchEvent(event);
                    return true;
                }})()"
            )
        };

        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn dispatch_regular_key(
        &self,
        key: &str,
        code: &str,
        is_down: bool,
        modifiers: &ModifierState,
    ) -> Result<(), WebDriverErrorResponse> {
        let ch = key.chars().next().unwrap_or(' ');
        let key_code = ch as u32;
        let event_type = if is_down { "keydown" } else { "keyup" };

        let escaped_key = key.replace('\\', "\\\\").replace('\'', "\\'");
        let escaped_code = code.replace('\\', "\\\\").replace('\'', "\\'");

        let ctrl_key = modifiers.ctrl;
        let meta_key = modifiers.meta;
        let shift_key = modifiers.shift;
        let alt_key = modifiers.alt;

        let is_select_all = is_down && (ch == 'a' || ch == 'A') && (ctrl_key || meta_key);

        let script = if is_select_all {
            format!(
                r"(function() {{
                    var activeEl = document.activeElement || document.body;

                    var keydownEvent = new KeyboardEvent('keydown', {{
                        key: '{escaped_key}',
                        code: '{escaped_code}',
                        keyCode: {key_code},
                        which: {key_code},
                        location: 0,
                        ctrlKey: {ctrl_key},
                        metaKey: {meta_key},
                        shiftKey: {shift_key},
                        altKey: {alt_key},
                        bubbles: true,
                        cancelable: true
                    }});
                    activeEl.dispatchEvent(keydownEvent);

                    if (activeEl.tagName === 'INPUT' || activeEl.tagName === 'TEXTAREA') {{
                        activeEl.select();
                    }} else {{
                        document.execCommand('selectAll', false, null);
                    }}

                    return true;
                }})()"
            )
        } else if is_down {
            format!(
                r"(function() {{
                    var activeEl = document.activeElement || document.body;

                    var keydownEvent = new KeyboardEvent('keydown', {{
                        key: '{escaped_key}',
                        code: '{escaped_code}',
                        keyCode: {key_code},
                        which: {key_code},
                        location: 0,
                        ctrlKey: {ctrl_key},
                        metaKey: {meta_key},
                        shiftKey: {shift_key},
                        altKey: {alt_key},
                        bubbles: true,
                        cancelable: true
                    }});
                    activeEl.dispatchEvent(keydownEvent);

                    if (!{ctrl_key} && !{meta_key} && !{alt_key}) {{
                        if (activeEl.tagName === 'INPUT' || activeEl.tagName === 'TEXTAREA') {{
                            var nativeInputValueSetter = Object.getOwnPropertyDescriptor(
                                activeEl.tagName === 'INPUT'
                                    ? window.HTMLInputElement.prototype
                                    : window.HTMLTextAreaElement.prototype,
                                'value'
                            ).set;

                            var newValue = activeEl.value + '{escaped_key}';
                            nativeInputValueSetter.call(activeEl, newValue);

                            var inputEvent = new InputEvent('input', {{
                                bubbles: true,
                                cancelable: true,
                                inputType: 'insertText',
                                data: '{escaped_key}'
                            }});
                            activeEl.dispatchEvent(inputEvent);
                        }}
                    }}

                    return true;
                }})()"
            )
        } else {
            format!(
                r"(function() {{
                    var activeEl = document.activeElement || document.body;
                    var event = new KeyboardEvent('{event_type}', {{
                        key: '{escaped_key}',
                        code: '{escaped_code}',
                        keyCode: {key_code},
                        which: {key_code},
                        location: 0,
                        ctrlKey: {ctrl_key},
                        metaKey: {meta_key},
                        shiftKey: {shift_key},
                        altKey: {alt_key},
                        bubbles: true,
                        cancelable: true
                    }});
                    activeEl.dispatchEvent(event);
                    return true;
                }})()"
            )
        };

        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn dispatch_pointer_event(
        &self,
        event_type: PointerEventType,
        x: i32,
        y: i32,
        button: u32,
        buttons: u32,
    ) -> Result<(), WebDriverErrorResponse> {
        let (pointer_event_name, mouse_event_name) = match event_type {
            PointerEventType::Down => (Some("pointerdown"), "mousedown"),
            PointerEventType::Up => (Some("pointerup"), "mouseup"),
            PointerEventType::Move => (Some("pointermove"), "mousemove"),
            // Manually dispatched mousedown/mouseup do NOT make the browser
            // synthesize a click, so element click handlers never fire. The
            // actions handler emits this explicitly after a same-spot down+up.
            PointerEventType::Click => (None, "click"),
        };

        let pointer_dispatch = pointer_event_name.map_or_else(String::new, |event_name| {
            format!(
                r"
                var pointerEvent = new PointerEvent('{event_name}', {{
                    bubbles: true,
                    cancelable: true,
                    clientX: {x},
                    clientY: {y},
                    button: {button},
                    buttons: {buttons},
                    pointerId: 1,
                    pointerType: 'mouse',
                    isPrimary: true
                }});
                if (!el.dispatchEvent(pointerEvent)) return true;
                "
            )
        });
        let script = format!(
            r"(function() {{
                var el = document.elementFromPoint({x}, {y});
                if (!el) el = document.body;

                {pointer_dispatch}

                var event = new MouseEvent('{mouse_event_name}', {{
                    bubbles: true,
                    cancelable: true,
                    clientX: {x},
                    clientY: {y},
                    button: {button},
                    buttons: {buttons}
                }});
                el.dispatchEvent(event);
                return true;
            }})()"
        );

        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn dispatch_scroll_event(
        &self,
        x: i32,
        y: i32,
        delta_x: i32,
        delta_y: i32,
    ) -> Result<(), WebDriverErrorResponse> {
        let script = format!(
            r"(function() {{
                var el = document.elementFromPoint({x}, {y});
                if (!el) el = document.body;

                var event = new WheelEvent('wheel', {{
                    bubbles: true,
                    cancelable: true,
                    clientX: {x},
                    clientY: {y},
                    deltaX: {delta_x},
                    deltaY: {delta_y},
                    deltaMode: 0
                }});
                el.dispatchEvent(event);

                window.scrollBy({delta_x}, {delta_y});
                return true;
            }})()"
        );

        self.evaluate_js(&script).await?;
        Ok(())
    }

    async fn get_window_rect(&self) -> Result<WindowRect, WebDriverErrorResponse> {
        if let Ok(position) = self.window().outer_position()
            && let Ok(size) = self.window().outer_size()
        {
            // The WebDriver specification defines window rects in CSS pixels,
            // while tao returns physical pixels. Returning the raw values
            // makes a 640 px request become a 320 CSS-pixel window on a 2x
            // display and similarly breaks mixed-DPI Windows setups.
            let scale_factor = self.window().scale_factor().unwrap_or(1.0);
            let position = position.to_logical::<i32>(scale_factor);
            let size = size.to_logical::<u32>(scale_factor);
            return Ok(WindowRect {
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
            });
        }
        Ok(WindowRect::default())
    }

    async fn set_window_rect(
        &self,
        rect: WindowRect,
    ) -> Result<WindowRect, WebDriverErrorResponse> {
        // Exit fullscreen/maximized state before setting rect
        // Otherwise the window manager may ignore our size/position request
        if self.window().is_fullscreen().unwrap_or(false) {
            let _ = self.window().set_fullscreen(false);
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        if self.window().is_maximized().unwrap_or(false) {
            let _ = self.window().unmaximize();
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }

        let _ = self
            .window()
            .set_position(LogicalPosition::new(rect.x, rect.y));

        let (chrome_width, chrome_height) = if let (Ok(outer), Ok(inner)) =
            (self.window().outer_size(), self.window().inner_size())
        {
            let scale_factor = self.window().scale_factor().unwrap_or(1.0);
            let outer = outer.to_logical::<u32>(scale_factor);
            let inner = inner.to_logical::<u32>(scale_factor);
            (
                outer.width.saturating_sub(inner.width),
                outer.height.saturating_sub(inner.height),
            )
        } else {
            (0, 0)
        };

        let inner_width = rect.width.saturating_sub(chrome_width);
        let inner_height = rect.height.saturating_sub(chrome_height);
        let _ = self
            .window()
            .set_size(LogicalSize::new(inner_width, inner_height));

        self.get_window_rect().await
    }

    async fn maximize_window(&self) -> Result<WindowRect, WebDriverErrorResponse> {
        let _ = self.window().maximize();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.get_window_rect().await
    }

    async fn minimize_window(&self) -> Result<(), WebDriverErrorResponse> {
        let _ = self.window().minimize();
        Ok(())
    }

    async fn fullscreen_window(&self) -> Result<WindowRect, WebDriverErrorResponse> {
        let _ = self.window().set_fullscreen(true);
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.get_window_rect().await
    }

    async fn switch_to_frame(&self, id: FrameId) -> Result<(), WebDriverErrorResponse> {
        match id {
            FrameId::Index(index) => {
                let script = format!(
                    r"(function() {{
                        var frames = document.querySelectorAll('iframe, frame');
                        if ({index} >= frames.length) {{
                            return false;
                        }}
                        return true;
                    }})()"
                );
                let result = self.evaluate_js(&script).await?;
                if result.get("value") == Some(&Value::Bool(false)) {
                    return Err(WebDriverErrorResponse::no_such_frame());
                }
                Ok(())
            }
            FrameId::Element(js_var) => {
                let script = format!(
                    r"(function() {{
                        var el = window.{js_var};
                        if (!el || !el.isConnected) {{
                            throw new Error('stale element reference');
                        }}
                        if (el.tagName !== 'IFRAME' && el.tagName !== 'FRAME') {{
                            throw new Error('element is not a frame');
                        }}
                        return true;
                    }})()"
                );
                self.evaluate_js(&script).await?;
                Ok(())
            }
        }
    }

    async fn switch_to_parent_frame(&self) -> Result<(), WebDriverErrorResponse> {
        // No-op - frame context is managed by the session, not the executor
        Ok(())
    }

    async fn get_all_cookies(&self) -> Result<Vec<Cookie>, WebDriverErrorResponse> {
        self.window()
            .cookies()
            .map(|cookies| cookies.iter().map(tauri_cookie_to_webdriver).collect())
            .map_err(|e| WebDriverErrorResponse::unknown_error(&e.to_string()))
    }

    async fn get_cookie(&self, name: &str) -> Result<Option<Cookie>, WebDriverErrorResponse> {
        let cookies = self.get_all_cookies().await?;
        Ok(cookies.into_iter().find(|c| c.name == name))
    }

    async fn add_cookie(&self, mut cookie: Cookie) -> Result<(), WebDriverErrorResponse> {
        // Per WebDriver spec: if no domain is specified, use the current page's domain
        if cookie.domain.is_none()
            && let Ok(url) = self.window().url()
        {
            cookie.domain = url.host_str().map(String::from);
        }

        if cookie.path.is_none() {
            cookie.path = Some("/".to_string());
        }

        let tauri_cookie = webdriver_cookie_to_tauri(&cookie);
        self.window()
            .set_cookie(tauri_cookie)
            .map_err(|e| WebDriverErrorResponse::unknown_error(&e.to_string()))
    }

    async fn delete_cookie(&self, name: &str) -> Result<(), WebDriverErrorResponse> {
        let cookies = self
            .window()
            .cookies()
            .map_err(|e| WebDriverErrorResponse::unknown_error(&e.to_string()))?;

        for cookie in cookies {
            if cookie.name() == name {
                self.window()
                    .delete_cookie(cookie)
                    .map_err(|e| WebDriverErrorResponse::unknown_error(&e.to_string()))?;
                return Ok(());
            }
        }
        Ok(())
    }

    async fn delete_all_cookies(&self) -> Result<(), WebDriverErrorResponse> {
        let cookies = self
            .window()
            .cookies()
            .map_err(|e| WebDriverErrorResponse::unknown_error(&e.to_string()))?;

        for cookie in cookies {
            self.window()
                .delete_cookie(cookie)
                .map_err(|e| WebDriverErrorResponse::unknown_error(&e.to_string()))?;
        }
        Ok(())
    }

    async fn dismiss_alert(&self) -> Result<(), WebDriverErrorResponse> {
        let manager = self.window().app_handle().state::<AlertStateManager>();
        let alert_state = manager.get_or_create(self.window().label());
        if alert_state.respond(false, None) {
            Ok(())
        } else {
            Err(WebDriverErrorResponse::no_such_alert())
        }
    }

    async fn accept_alert(&self) -> Result<(), WebDriverErrorResponse> {
        let manager = self.window().app_handle().state::<AlertStateManager>();
        let alert_state = manager.get_or_create(self.window().label());
        let prompt_text = alert_state
            .get_prompt_input()
            .or_else(|| alert_state.get_default_text());
        if alert_state.respond(true, prompt_text) {
            Ok(())
        } else {
            Err(WebDriverErrorResponse::no_such_alert())
        }
    }

    async fn get_alert_text(&self) -> Result<String, WebDriverErrorResponse> {
        let manager = self.window().app_handle().state::<AlertStateManager>();
        let alert_state = manager.get_or_create(self.window().label());
        match alert_state.get_message() {
            Some(msg) => Ok(msg),
            None => Err(WebDriverErrorResponse::no_such_alert()),
        }
    }

    async fn send_alert_text(&self, text: &str) -> Result<(), WebDriverErrorResponse> {
        let manager = self.window().app_handle().state::<AlertStateManager>();
        let alert_state = manager.get_or_create(self.window().label());
        match alert_state.get_alert_type() {
            None => Err(WebDriverErrorResponse::no_such_alert()),
            Some(AlertType::Prompt) => {
                if alert_state.set_prompt_input(text.to_string()) {
                    Ok(())
                } else {
                    Err(WebDriverErrorResponse::no_such_alert())
                }
            }
            Some(_) => Err(WebDriverErrorResponse::element_not_interactable(
                "User prompt is not a prompt dialog",
            )),
        }
    }

    async fn print_page(&self, options: PrintOptions) -> Result<String, WebDriverErrorResponse>;
}

fn crop_screenshot(
    screenshot: &str,
    rect: &ScreenshotRect,
) -> Result<String, WebDriverErrorResponse> {
    if ![
        rect.x,
        rect.y,
        rect.width,
        rect.height,
        rect.viewport_width,
        rect.viewport_height,
    ]
    .iter()
    .all(|value| value.is_finite())
        || rect.width <= 0.0
        || rect.height <= 0.0
        || rect.viewport_width <= 0.0
        || rect.viewport_height <= 0.0
    {
        return Err(WebDriverErrorResponse::unable_to_capture_screen(
            "Invalid element screenshot rectangle",
        ));
    }

    let png = BASE64_STANDARD.decode(screenshot).map_err(|error| {
        WebDriverErrorResponse::unable_to_capture_screen(&format!(
            "Invalid screenshot data: {error}"
        ))
    })?;
    let image = image::load_from_memory_with_format(&png, ImageFormat::Png).map_err(|error| {
        WebDriverErrorResponse::unable_to_capture_screen(&format!(
            "Invalid screenshot PNG: {error}"
        ))
    })?;

    let scale_x = f64::from(image.width()) / rect.viewport_width;
    let scale_y = f64::from(image.height()) / rect.viewport_height;
    let left = (rect.x * scale_x).floor().max(0.0) as u32;
    let top = (rect.y * scale_y).floor().max(0.0) as u32;
    let width = (rect.width * scale_x).floor().max(0.0) as u32;
    let height = (rect.height * scale_y).floor().max(0.0) as u32;
    let right = left.saturating_add(width).min(image.width());
    let bottom = top.saturating_add(height).min(image.height());

    if right <= left || bottom <= top {
        return Err(WebDriverErrorResponse::unable_to_capture_screen(
            "Element screenshot rectangle is outside the viewport",
        ));
    }

    let cropped = image.crop_imm(left, top, right - left, bottom - top);
    let mut output = Cursor::new(Vec::new());
    cropped
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|error| {
            WebDriverErrorResponse::unable_to_capture_screen(&format!(
                "Failed to encode element screenshot: {error}"
            ))
        })?;
    Ok(BASE64_STANDARD.encode(output.into_inner()))
}

fn extract_string_value(result: &Value) -> Result<String, WebDriverErrorResponse> {
    if let Some(success) = result.get("success").and_then(Value::as_bool) {
        if success {
            if let Some(value) = result.get("value") {
                if let Some(s) = value.as_str() {
                    return Ok(s.to_string());
                }
                return Ok(value.to_string());
            }
        } else if let Some(error) = result.get("error").and_then(Value::as_str) {
            return Err(WebDriverErrorResponse::javascript_error(error, None));
        }
    }
    Ok(String::new())
}

fn extract_bool_value(result: &Value) -> Result<bool, WebDriverErrorResponse> {
    if let Some(success) = result.get("success").and_then(Value::as_bool) {
        if success {
            if let Some(value) = result.get("value").and_then(Value::as_bool) {
                return Ok(value);
            }
        } else if let Some(error) = result.get("error").and_then(Value::as_str) {
            return Err(WebDriverErrorResponse::javascript_error(error, None));
        }
    }
    Ok(false)
}

fn extract_usize_value(result: &Value) -> Result<usize, WebDriverErrorResponse> {
    if let Some(success) = result.get("success").and_then(Value::as_bool) {
        if success {
            if let Some(count) = result.get("value").and_then(Value::as_u64) {
                return Ok(usize::try_from(count).unwrap_or(0));
            }
        } else if let Some(error) = result.get("error").and_then(Value::as_str) {
            return Err(WebDriverErrorResponse::javascript_error(error, None));
        }
    }
    Ok(0)
}

fn extract_value(result: &Value) -> Result<Value, WebDriverErrorResponse> {
    if let Some(success) = result.get("success").and_then(Value::as_bool) {
        if success {
            return Ok(result.get("value").cloned().unwrap_or(Value::Null));
        } else if let Some(error) = result.get("error").and_then(Value::as_str) {
            return Err(WebDriverErrorResponse::javascript_error(error, None));
        }
    }
    Ok(Value::Null)
}

fn extract_script_result_from_inner(inner: &Value) -> Result<Value, WebDriverErrorResponse> {
    if let Some(success) = inner.get("__wd_success").and_then(Value::as_bool) {
        if success {
            return Ok(inner.get("__wd_value").cloned().unwrap_or(Value::Null));
        } else if let Some(error) = inner.get("__wd_error").and_then(Value::as_str) {
            return Err(WebDriverErrorResponse::javascript_error(error, None));
        }
    }

    // If we got null or no wrapper structure, it's likely a syntax error
    if inner.is_null() || inner.get("__wd_success").is_none() {
        return Err(WebDriverErrorResponse::javascript_error(
            "Script execution failed (possible syntax error)",
            None,
        ));
    }

    Ok(Value::Null)
}

/// Wrap a JavaScript script to execute within a specific frame context.
/// If `frame_context` is empty (top-level), returns the script unchanged.
/// Otherwise, wraps the script to navigate to the correct frame before execution.
pub fn wrap_script_for_frame_context(script: &str, frame_context: &[FrameId]) -> String {
    use std::fmt::Write;

    if frame_context.is_empty() {
        return script.to_string();
    }

    let mut frame_nav = String::new();
    frame_nav.push_str("(function() {\n");
    frame_nav.push_str("  var ctx = window;\n");
    frame_nav.push_str("  var doc = document;\n");

    for (i, frame_id) in frame_context.iter().enumerate() {
        match frame_id {
            FrameId::Index(index) => {
                let _ = writeln!(
                    frame_nav,
                    "  var frames{i} = doc.querySelectorAll('iframe, frame');"
                );
                let _ = writeln!(
                    frame_nav,
                    "  if ({index} >= frames{i}.length) throw new Error('no such frame');"
                );
                let _ = writeln!(frame_nav, "  var frame{i} = frames{i}[{index}];");
                let _ = writeln!(
                    frame_nav,
                    "  if (!frame{i}.contentWindow) throw new Error('no such frame');"
                );
                let _ = writeln!(frame_nav, "  ctx = frame{i}.contentWindow;");
                let _ = writeln!(frame_nav, "  doc = frame{i}.contentDocument;");
            }
            FrameId::Element(js_var) => {
                let _ = writeln!(frame_nav, "  var frame{i} = window.{js_var};");
                let _ = writeln!(
                    frame_nav,
                    "  if (!frame{i} || !doc.contains(frame{i})) throw new Error('stale element reference');"
                );
                let _ = writeln!(
                    frame_nav,
                    "  if (frame{i}.tagName !== 'IFRAME' && frame{i}.tagName !== 'FRAME') throw new Error('element is not a frame');"
                );
                let _ = writeln!(
                    frame_nav,
                    "  if (!frame{i}.contentWindow) throw new Error('no such frame');"
                );
                let _ = writeln!(frame_nav, "  ctx = frame{i}.contentWindow;");
                let _ = writeln!(frame_nav, "  doc = frame{i}.contentDocument;");
            }
        }
    }

    let escaped_script = script
        .replace('\\', "\\\\")
        .replace('`', "\\`")
        .replace("${", "\\${");

    let _ = writeln!(frame_nav, "  return ctx.eval(`{escaped_script}`);");
    frame_nav.push_str("})()");

    frame_nav
}

fn tauri_cookie_to_webdriver(cookie: &TauriCookie<'static>) -> Cookie {
    use tauri::webview::cookie::{Expiration, SameSite};

    Cookie {
        name: cookie.name().to_string(),
        value: cookie.value().to_string(),
        path: cookie.path().map(String::from),
        domain: cookie.domain().map(String::from),
        secure: cookie.secure().unwrap_or(false),
        http_only: cookie.http_only().unwrap_or(false),
        expiry: cookie.expires().and_then(|exp| match exp {
            Expiration::DateTime(dt) => Some(dt.unix_timestamp().cast_unsigned()),
            Expiration::Session => None,
        }),
        same_site: cookie.same_site().map(|ss| match ss {
            SameSite::Strict => "Strict".to_string(),
            SameSite::Lax => "Lax".to_string(),
            SameSite::None => "None".to_string(),
        }),
    }
}

fn webdriver_cookie_to_tauri(cookie: &Cookie) -> TauriCookie<'static> {
    use tauri::webview::cookie::{Expiration, SameSite, time::OffsetDateTime};

    let mut builder = TauriCookie::build((cookie.name.clone(), cookie.value.clone()));

    if let Some(ref path) = cookie.path {
        builder = builder.path(path.clone());
    }

    if let Some(ref domain) = cookie.domain {
        builder = builder.domain(domain.clone());
    }

    builder = builder.secure(cookie.secure);

    if cookie.http_only {
        builder = builder.http_only(true);
    }

    if let Some(expiry) = cookie.expiry
        && let Ok(dt) = OffsetDateTime::from_unix_timestamp(expiry.cast_signed())
    {
        builder = builder.expires(Expiration::DateTime(dt));
    }

    if let Some(ref same_site) = cookie.same_site {
        let ss = match same_site.to_lowercase().as_str() {
            "strict" => SameSite::Strict,
            "lax" => SameSite::Lax,
            _ => SameSite::None,
        };
        builder = builder.same_site(ss);
    }

    builder.build()
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use base64::Engine as _;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use image::{DynamicImage, ImageBuffer, ImageFormat, Rgba};

    use super::{
        ModifierState, ScreenshotRect, await_script_timeout, crop_screenshot, poll_implicit,
        webdriver_key_event,
    };

    #[test]
    fn webdriver_special_keys_use_wpt_key_code_and_location() {
        let return_key = webdriver_key_event("\u{E006}").unwrap();
        assert_eq!(
            (return_key.key, return_key.code, return_key.location),
            ("Enter", "Enter", 0)
        );

        let enter = webdriver_key_event("\u{E007}").unwrap();
        assert_eq!(
            (enter.key, enter.code, enter.location),
            ("Enter", "NumpadEnter", 1)
        );

        let right_shift = webdriver_key_event("\u{E050}").unwrap();
        assert_eq!(
            (right_shift.key, right_shift.code, right_shift.location),
            ("Shift", "ShiftRight", 2)
        );

        let numpad_page_up = webdriver_key_event("\u{E054}").unwrap();
        assert_eq!(
            (
                numpad_page_up.key,
                numpad_page_up.code,
                numpad_page_up.location
            ),
            ("PageUp", "Numpad9", 3)
        );

        assert_eq!(webdriver_key_event("\u{E001}").unwrap().code, "");
        assert_eq!(webdriver_key_event("\u{E005}").unwrap().code, "");
        assert_eq!(webdriver_key_event("\u{E018}").unwrap().code, "");
        assert_eq!(webdriver_key_event("\u{E019}").unwrap().code, "NumpadEqual");
        assert_eq!(webdriver_key_event("\u{E040}").unwrap().code, "");
        let null = webdriver_key_event("\u{E000}").unwrap();
        assert_eq!(
            (null.key, null.code, null.location),
            ("Unidentified", "", 0)
        );

        let mut modifiers = ModifierState::default();
        modifiers.update("\u{E050}", true);
        modifiers.update("\u{E051}", true);
        modifiers.update("\u{E052}", true);
        modifiers.update("\u{E053}", true);
        assert!(modifiers.shift && modifiers.ctrl && modifiers.alt && modifiers.meta);
    }

    #[tokio::test]
    async fn implicit_poll_retries_until_a_result_is_found() {
        let attempts = AtomicUsize::new(0);
        let found = poll_implicit(
            Some(500),
            || async { Ok::<_, ()>(attempts.fetch_add(1, Ordering::SeqCst) >= 2) },
            |found| *found,
        )
        .await
        .unwrap();

        assert!(found);
        assert_eq!(attempts.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn zero_implicit_timeout_still_performs_one_lookup() {
        let attempts = AtomicUsize::new(0);
        let found = poll_implicit(
            Some(0),
            || async {
                attempts.fetch_add(1, Ordering::SeqCst);
                Ok::<_, ()>(false)
            },
            |found| *found,
        )
        .await
        .unwrap();

        assert!(!found);
        assert_eq!(attempts.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn null_script_timeout_waits_without_a_webdriver_deadline() {
        let value = await_script_timeout(None, async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            7
        })
        .await;
        assert_eq!(value, Ok(7));

        let timed_out = await_script_timeout(Some(1), async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        })
        .await;
        assert!(timed_out.is_err());
    }

    #[test]
    fn element_screenshot_is_cropped_in_css_pixel_coordinates() {
        let image = ImageBuffer::from_fn(200, 160, |x, y| {
            Rgba([(x % 255) as u8, (y % 255) as u8, 0, 255])
        });
        let mut png = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(image)
            .write_to(&mut png, ImageFormat::Png)
            .unwrap();

        let screenshot = BASE64_STANDARD.encode(png.into_inner());
        let cropped = crop_screenshot(
            &screenshot,
            &ScreenshotRect {
                x: 10.0,
                y: 15.0,
                width: 30.0,
                height: 20.0,
                viewport_width: 100.0,
                viewport_height: 80.0,
            },
        )
        .unwrap();
        let decoded = BASE64_STANDARD.decode(cropped).unwrap();
        let image = image::load_from_memory_with_format(&decoded, ImageFormat::Png).unwrap();

        assert_eq!((image.width(), image.height()), (60, 40));
        assert_eq!(image.to_rgba8().get_pixel(0, 0), &Rgba([20, 30, 0, 255]));

        let fractional = crop_screenshot(
            &screenshot,
            &ScreenshotRect {
                x: 10.25,
                y: 15.75,
                width: 30.25,
                height: 20.25,
                viewport_width: 200.0,
                viewport_height: 160.0,
            },
        )
        .unwrap();
        let decoded = BASE64_STANDARD.decode(fractional).unwrap();
        let image = image::load_from_memory_with_format(&decoded, ImageFormat::Png).unwrap();
        assert_eq!((image.width(), image.height()), (30, 20));

        let error = crop_screenshot(
            "not-base64",
            &ScreenshotRect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
                viewport_width: 1.0,
                viewport_height: 1.0,
            },
        )
        .unwrap_err();
        assert_eq!(error.error, "unable to capture screen");
    }
}
