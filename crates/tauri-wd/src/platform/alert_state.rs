//! Cross-platform alert state management for `WebDriver`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertType {
    Alert,
    Confirm,
    Prompt,
}

#[derive(Debug, Clone)]
pub struct AlertResponse {
    pub accepted: bool,
    pub prompt_text: Option<String>,
}

pub struct PendingAlert {
    pub message: String,
    pub default_text: Option<String>,
    pub alert_type: AlertType,
    pub responder: std::sync::mpsc::Sender<AlertResponse>,
}

/// Per-window alert state for coordinating between UI delegate and `WebDriver` commands
pub struct AlertState {
    pending: Mutex<Option<PendingAlert>>,
    prompt_input: Mutex<Option<String>>,
}

impl AlertState {
    pub fn new() -> Self {
        Self {
            pending: Mutex::new(None),
            prompt_input: Mutex::new(None),
        }
    }

    pub fn set_pending(&self, alert: PendingAlert) {
        if let Ok(mut guard) = self.prompt_input.lock() {
            *guard = None;
        }
        if let Ok(mut guard) = self.pending.lock() {
            *guard = Some(alert);
        }
    }

    pub fn set_prompt_input(&self, text: String) -> bool {
        if !matches!(self.get_alert_type(), Some(AlertType::Prompt)) {
            return false;
        }
        if let Ok(mut guard) = self.prompt_input.lock() {
            *guard = Some(text);
            true
        } else {
            false
        }
    }

    pub fn get_prompt_input(&self) -> Option<String> {
        if let Ok(guard) = self.prompt_input.lock() {
            guard.clone()
        } else {
            None
        }
    }

    pub fn get_message(&self) -> Option<String> {
        if let Ok(guard) = self.pending.lock() {
            guard.as_ref().map(|a| a.message.clone())
        } else {
            None
        }
    }

    pub fn get_alert_type(&self) -> Option<AlertType> {
        if let Ok(guard) = self.pending.lock() {
            guard.as_ref().map(|a| a.alert_type)
        } else {
            None
        }
    }

    pub fn get_default_text(&self) -> Option<String> {
        if let Ok(guard) = self.pending.lock() {
            guard.as_ref().and_then(|a| a.default_text.clone())
        } else {
            None
        }
    }

    pub fn respond(&self, accepted: bool, prompt_text: Option<String>) -> bool {
        if let Ok(mut guard) = self.pending.lock()
            && let Some(alert) = guard.take()
        {
            if let Ok(mut input_guard) = self.prompt_input.lock() {
                *input_guard = None;
            }
            let _ = alert.responder.send(AlertResponse {
                accepted,
                prompt_text,
            });
            return true;
        }
        false
    }
}

impl Default for AlertState {
    fn default() -> Self {
        Self::new()
    }
}

pub struct AlertStateManager {
    states: Mutex<HashMap<String, Arc<AlertState>>>,
}

impl AlertStateManager {
    pub fn new() -> Self {
        Self {
            states: Mutex::new(HashMap::new()),
        }
    }

    pub fn get_or_create(&self, window_label: &str) -> Arc<AlertState> {
        let mut states = self.states.lock().expect("AlertStateManager lock poisoned");
        states
            .entry(window_label.to_string())
            .or_insert_with(|| Arc::new(AlertState::new()))
            .clone()
    }
}

impl Default for AlertStateManager {
    fn default() -> Self {
        Self::new()
    }
}
