use std::collections::{HashMap, HashSet};

use serde::Serialize;
use uuid::Uuid;

use super::element::ElementStore;
use crate::platform::FrameId;
use crate::server::response::WebDriverErrorResponse;

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum PageLoadStrategy {
    None,
    Eager,
    #[default]
    Normal,
}

impl PageLoadStrategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Eager => "eager",
            Self::Normal => "normal",
        }
    }
}

#[derive(Debug, Default)]
pub struct ActionState {
    pub pressed_keys: HashSet<String>,
    pub pressed_buttons: HashMap<String, HashSet<u32>>,
    pub pointer_positions: HashMap<String, (i32, i32)>,
    pub primary_down_positions: HashMap<String, (i32, i32)>,
}

#[derive(Debug, Clone, Serialize)]
#[allow(clippy::struct_field_names)]
pub struct Timeouts {
    pub implicit_ms: Option<u64>,
    pub page_load_ms: Option<u64>,
    pub script_ms: Option<u64>,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            implicit_ms: Some(0),
            page_load_ms: Some(300_000),
            script_ms: Some(30_000),
        }
    }
}

#[derive(Debug)]
pub struct Session {
    pub id: String,
    pub timeouts: Timeouts,
    pub elements: ElementStore,
    pub current_window: String,
    pub frame_context: Vec<FrameId>,
    pub action_state: ActionState,
    pub page_load_strategy: PageLoadStrategy,
}

impl Session {
    pub fn new(
        initial_window: String,
        page_load_strategy: PageLoadStrategy,
        timeouts: Timeouts,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            timeouts,
            elements: ElementStore::new(),
            current_window: initial_window,
            frame_context: Vec::new(),
            action_state: ActionState::default(),
            page_load_strategy,
        }
    }
}

#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: HashMap<String, Session>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    pub fn create(
        &mut self,
        initial_window: String,
        page_load_strategy: PageLoadStrategy,
        timeouts: Timeouts,
    ) -> &Session {
        let session = Session::new(initial_window, page_load_strategy, timeouts);
        let id = session.id.clone();
        self.sessions.insert(id.clone(), session);
        self.sessions.get(&id).expect("session was just inserted")
    }

    pub fn get(&self, id: &str) -> Result<&Session, WebDriverErrorResponse> {
        self.sessions
            .get(id)
            .ok_or_else(|| WebDriverErrorResponse::invalid_session_id(id))
    }

    pub fn get_mut(&mut self, id: &str) -> Result<&mut Session, WebDriverErrorResponse> {
        self.sessions
            .get_mut(id)
            .ok_or_else(|| WebDriverErrorResponse::invalid_session_id(id))
    }

    pub fn delete(&mut self, id: &str) -> bool {
        self.sessions.remove(id).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }
}
