use std::collections::HashMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use serde::Deserialize;
use tauri::Runtime;

use crate::platform::{ModifierState, PointerEventType};
use crate::server::AppState;
use crate::server::response::{WebDriverErrorResponse, WebDriverResponse, WebDriverResult};

#[derive(Debug, Deserialize)]
pub struct ActionsRequest {
    pub actions: Vec<ActionSequence>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ActionSequence {
    #[serde(rename = "key")]
    Key {
        #[serde(rename = "id")]
        _id: String,
        actions: Vec<KeyAction>,
    },
    #[serde(rename = "pointer")]
    Pointer {
        id: String,
        actions: Vec<PointerAction>,
    },
    #[serde(rename = "wheel")]
    Wheel {
        #[serde(rename = "id")]
        _id: String,
        actions: Vec<WheelAction>,
    },
    #[serde(rename = "none")]
    None {
        #[serde(rename = "id")]
        _id: String,
        actions: Vec<PauseAction>,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum KeyAction {
    #[serde(rename = "keyDown")]
    KeyDown { value: String },
    #[serde(rename = "keyUp")]
    KeyUp { value: String },
    #[serde(rename = "pause")]
    Pause { duration: Option<u64> },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum PointerAction {
    #[serde(rename = "pointerDown")]
    PointerDown { button: u32 },
    #[serde(rename = "pointerUp")]
    PointerUp { button: u32 },
    #[serde(rename = "pointerMove")]
    PointerMove {
        x: i32,
        y: i32,
        duration: Option<u64>,
        #[serde(default)]
        origin: Option<Origin>,
    },
    #[serde(rename = "pause")]
    Pause { duration: Option<u64> },
}

/// W3C JSON key identifying a web element reference.
const ELEMENT_KEY: &str = "element-6066-11e4-a52e-4f735466cecf";

/// Coordinate origin for a `pointerMove`. Per the WebDriver Actions spec the
/// `origin` is either the string `"viewport"` (the default — x/y are absolute
/// viewport coordinates) or `"pointer"` (x/y are relative to the current pointer
/// position), or an element reference object
/// `{ "element-6066-11e4-a52e-4f735466cecf": "<id>" }` (x/y are offsets from the
/// element's in-view center point). Some clients send the element form for
/// `element.click(options)` with x/y defaulting to 0, so this must resolve to the
/// element's center rather than viewport (0,0).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum Origin {
    Named(String),
    Element(HashMap<String, String>),
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum WheelAction {
    #[serde(rename = "scroll")]
    Scroll {
        x: i32,
        y: i32,
        #[serde(rename = "deltaX")]
        delta_x: i32,
        #[serde(rename = "deltaY")]
        delta_y: i32,
        #[serde(default)]
        duration: Option<u64>,
    },
    #[serde(rename = "pause")]
    Pause { duration: Option<u64> },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum PauseAction {
    #[serde(rename = "pause")]
    Pause { duration: Option<u64> },
}

struct PointerState {
    x: i32,
    y: i32,
}

struct PendingPointerMove {
    start_x: i32,
    start_y: i32,
    target_x: i32,
    target_y: i32,
    duration_ms: u64,
    buttons: u32,
}

impl ActionSequence {
    fn len(&self) -> usize {
        match self {
            Self::Key { actions, .. } => actions.len(),
            Self::Pointer { actions, .. } => actions.len(),
            Self::Wheel { actions, .. } => actions.len(),
            Self::None { actions, .. } => actions.len(),
        }
    }

    fn duration_at(&self, tick: usize) -> u64 {
        match self {
            Self::Key { actions, .. } => actions.get(tick).and_then(|action| match action {
                KeyAction::Pause { duration } => *duration,
                _ => None,
            }),
            Self::Pointer { actions, .. } => actions.get(tick).and_then(|action| match action {
                PointerAction::PointerMove { duration, .. } | PointerAction::Pause { duration } => {
                    *duration
                }
                _ => None,
            }),
            Self::Wheel { actions, .. } => actions.get(tick).and_then(|action| match action {
                WheelAction::Scroll { duration, .. } | WheelAction::Pause { duration } => *duration,
            }),
            Self::None { actions, .. } => actions.get(tick).and_then(|action| match action {
                PauseAction::Pause { duration } => *duration,
            }),
        }
        .unwrap_or(0)
    }
}

fn button_mask(button: u32) -> u32 {
    match button {
        0 => 1,
        1 => 4,
        2 => 2,
        3 => 8,
        4 => 16,
        _ => 0,
    }
}

fn buttons_mask(buttons: &std::collections::HashSet<u32>) -> u32 {
    buttons
        .iter()
        .fold(0, |mask, button| mask | button_mask(*button))
}

#[allow(clippy::too_many_lines)]
pub async fn perform<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
    Json(request): Json<ActionsRequest>,
) -> WebDriverResult {
    let (
        current_window,
        timeouts,
        frame_context,
        mut pointer_positions,
        mut primary_down_positions,
        mut pressed_keys,
    ) = {
        let sessions = state.sessions.read().await;
        let session = sessions.get(&session_id)?;
        (
            session.current_window.clone(),
            session.timeouts.clone(),
            session.frame_context.clone(),
            session.action_state.pointer_positions.clone(),
            session.action_state.primary_down_positions.clone(),
            session.action_state.pressed_keys.clone(),
        )
    };

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let mut modifier_state = ModifierState::default();
    for key in &pressed_keys {
        modifier_state.update(key, true);
    }

    let tick_count = request
        .actions
        .iter()
        .map(ActionSequence::len)
        .max()
        .unwrap_or(0);
    for tick in 0..tick_count {
        let tick_duration = request
            .actions
            .iter()
            .map(|sequence| sequence.duration_at(tick))
            .max()
            .unwrap_or(0);
        let tick_start = tokio::time::Instant::now();
        let mut pending_pointer_moves = Vec::new();

        for action_seq in &request.actions {
            match action_seq {
                ActionSequence::Key { _id: _, actions } => {
                    if let Some(action) = actions.get(tick) {
                        match action {
                            KeyAction::KeyDown { value } => {
                                modifier_state.update(value, true);
                                executor
                                    .dispatch_key_event(value, true, &modifier_state)
                                    .await?;
                                pressed_keys.insert(value.clone());
                                let mut sessions = state.sessions.write().await;
                                if let Ok(session) = sessions.get_mut(&session_id) {
                                    session.action_state.pressed_keys.insert(value.clone());
                                }
                            }
                            KeyAction::KeyUp { value } => {
                                if pressed_keys.remove(value) {
                                    modifier_state.update(value, false);
                                    executor
                                        .dispatch_key_event(value, false, &modifier_state)
                                        .await?;
                                    let mut sessions = state.sessions.write().await;
                                    if let Ok(session) = sessions.get_mut(&session_id) {
                                        session.action_state.pressed_keys.remove(value);
                                    }
                                }
                            }
                            KeyAction::Pause { .. } => {}
                        }
                    }
                }
                ActionSequence::Pointer { id, actions } => {
                    let position = pointer_positions.get(id).copied().unwrap_or_default();
                    let mut pointer_state = PointerState {
                        x: position.0,
                        y: position.1,
                    };
                    let mut primary_down_pos = primary_down_positions.get(id).copied();
                    let mut pressed_buttons = {
                        let sessions = state.sessions.read().await;
                        sessions
                            .get(&session_id)?
                            .action_state
                            .pressed_buttons
                            .get(id)
                            .cloned()
                            .unwrap_or_default()
                    };
                    if let Some(action) = actions.get(tick) {
                        match action {
                            PointerAction::PointerDown { button } => {
                                pressed_buttons.insert(*button);
                                executor
                                    .dispatch_pointer_event(
                                        PointerEventType::Down,
                                        pointer_state.x,
                                        pointer_state.y,
                                        *button,
                                        buttons_mask(&pressed_buttons),
                                    )
                                    .await?;
                                if *button == 0 {
                                    primary_down_pos = Some((pointer_state.x, pointer_state.y));
                                }
                                let mut sessions = state.sessions.write().await;
                                if let Ok(session) = sessions.get_mut(&session_id) {
                                    session
                                        .action_state
                                        .pressed_buttons
                                        .entry(id.clone())
                                        .or_default()
                                        .insert(*button);
                                }
                            }
                            PointerAction::PointerUp { button } => {
                                pressed_buttons.remove(button);
                                executor
                                    .dispatch_pointer_event(
                                        PointerEventType::Up,
                                        pointer_state.x,
                                        pointer_state.y,
                                        *button,
                                        buttons_mask(&pressed_buttons),
                                    )
                                    .await?;
                                // A primary press + release on the same spot is a
                                // click; emit the click event the browser would
                                // synthesize for real input so element handlers fire.
                                // Only the primary button's release consumes/clears
                                // the press state — a non-primary release in between
                                // must not drop it.
                                if *button == 0 {
                                    if primary_down_pos == Some((pointer_state.x, pointer_state.y))
                                    {
                                        executor
                                            .dispatch_pointer_event(
                                                PointerEventType::Click,
                                                pointer_state.x,
                                                pointer_state.y,
                                                *button,
                                                buttons_mask(&pressed_buttons),
                                            )
                                            .await?;
                                    }
                                    primary_down_pos = None;
                                }
                                let mut sessions = state.sessions.write().await;
                                if let Ok(session) = sessions.get_mut(&session_id)
                                    && let Some(buttons) =
                                        session.action_state.pressed_buttons.get_mut(id)
                                {
                                    buttons.remove(button);
                                }
                            }
                            PointerAction::PointerMove {
                                x,
                                y,
                                duration,
                                origin,
                            } => {
                                let start_x = pointer_state.x;
                                let start_y = pointer_state.y;
                                let (target_x, target_y) = match origin {
                                    // No origin (the default) or "viewport": x/y are absolute viewport coords.
                                    None => (*x, *y),
                                    Some(Origin::Named(name)) if name == "viewport" => (*x, *y),
                                    Some(Origin::Named(name)) if name == "pointer" => {
                                        (pointer_state.x + *x, pointer_state.y + *y)
                                    }
                                    // The spec defines only "viewport" and "pointer" as named origins;
                                    // reject anything else rather than silently treating it as viewport.
                                    Some(Origin::Named(name)) => {
                                        return Err(WebDriverErrorResponse::invalid_argument(
                                            &format!(
                                                "pointerMove origin '{name}' is not a recognised named origin (expected 'viewport' or 'pointer')"
                                            ),
                                        ));
                                    }
                                    Some(Origin::Element(refs)) => {
                                        let element_id = refs.get(ELEMENT_KEY).ok_or_else(|| {
                                        WebDriverErrorResponse::invalid_argument(
                                            "pointerMove origin is missing a web element reference",
                                        )
                                    })?;
                                        let js_var = {
                                            let sessions = state.sessions.read().await;
                                            let session = sessions.get(&session_id)?;
                                            session
                                                .elements
                                                .get(element_id)
                                                .ok_or_else(
                                                    WebDriverErrorResponse::no_such_element,
                                                )?
                                                .js_ref
                                                .clone()
                                        };
                                        let (cx, cy) = executor.get_element_center(&js_var).await?;
                                        (cx + *x, cy + *y)
                                    }
                                };
                                pointer_state.x = target_x;
                                pointer_state.y = target_y;
                                let duration_ms = duration.unwrap_or(0);
                                if duration_ms == 0 {
                                    executor
                                        .dispatch_pointer_event(
                                            PointerEventType::Move,
                                            pointer_state.x,
                                            pointer_state.y,
                                            0,
                                            buttons_mask(&pressed_buttons),
                                        )
                                        .await?;
                                } else {
                                    pending_pointer_moves.push(PendingPointerMove {
                                        start_x,
                                        start_y,
                                        target_x,
                                        target_y,
                                        duration_ms,
                                        buttons: buttons_mask(&pressed_buttons),
                                    });
                                }
                            }
                            PointerAction::Pause { .. } => {}
                        }
                    }
                    pointer_positions.insert(id.clone(), (pointer_state.x, pointer_state.y));
                    if let Some(position) = primary_down_pos {
                        primary_down_positions.insert(id.clone(), position);
                    } else {
                        primary_down_positions.remove(id);
                    }
                }
                ActionSequence::Wheel { _id: _, actions } => {
                    if let Some(action) = actions.get(tick) {
                        match action {
                            WheelAction::Scroll {
                                x,
                                y,
                                delta_x,
                                delta_y,
                                duration: _,
                            } => {
                                executor
                                    .dispatch_scroll_event(*x, *y, *delta_x, *delta_y)
                                    .await?;
                            }
                            WheelAction::Pause { .. } => {}
                        }
                    }
                }
                ActionSequence::None { _id: _, actions } => {
                    if let Some(action) = actions.get(tick) {
                        match action {
                            PauseAction::Pause { .. } => {}
                        }
                    }
                }
            }
        }

        if let Some(max_move_duration) = pending_pointer_moves
            .iter()
            .map(|movement| movement.duration_ms)
            .max()
        {
            let mut previous_elapsed_ms = 0;
            let mut elapsed_ms = 16_u64.min(max_move_duration);
            loop {
                let deadline = tick_start + std::time::Duration::from_millis(elapsed_ms);
                tokio::time::sleep_until(deadline).await;

                for movement in &pending_pointer_moves {
                    if previous_elapsed_ms >= movement.duration_ms {
                        continue;
                    }
                    let movement_elapsed_ms = elapsed_ms.min(movement.duration_ms);
                    let progress = movement_elapsed_ms as f64 / movement.duration_ms as f64;
                    let x = f64::from(movement.start_x)
                        + f64::from(movement.target_x - movement.start_x) * progress;
                    let y = f64::from(movement.start_y)
                        + f64::from(movement.target_y - movement.start_y) * progress;
                    executor
                        .dispatch_pointer_event(
                            PointerEventType::Move,
                            x.round() as i32,
                            y.round() as i32,
                            0,
                            movement.buttons,
                        )
                        .await?;
                }

                if elapsed_ms == max_move_duration {
                    break;
                }
                previous_elapsed_ms = elapsed_ms;
                elapsed_ms = (elapsed_ms + 16).min(max_move_duration);
            }
        }

        let tick_duration = std::time::Duration::from_millis(tick_duration);
        let elapsed = tick_start.elapsed();
        if elapsed < tick_duration {
            tokio::time::sleep(tick_duration - elapsed).await;
        }
    }

    {
        let mut sessions = state.sessions.write().await;
        if let Ok(session) = sessions.get_mut(&session_id) {
            session.action_state.pointer_positions = pointer_positions;
            session.action_state.primary_down_positions = primary_down_positions;
        }
    }

    Ok(WebDriverResponse::null())
}

pub async fn release<R: Runtime + 'static>(
    State(state): State<Arc<AppState<R>>>,
    Path(session_id): Path<String>,
) -> WebDriverResult {
    let (current_window, timeouts, frame_context, pressed_keys, pressed_buttons, pointer_positions) = {
        let mut sessions = state.sessions.write().await;
        let session = sessions.get_mut(&session_id)?;
        let pressed_keys: Vec<String> = session.action_state.pressed_keys.drain().collect();
        let pressed_buttons = std::mem::take(&mut session.action_state.pressed_buttons);
        let pointer_positions = std::mem::take(&mut session.action_state.pointer_positions);
        session.action_state.primary_down_positions.clear();
        (
            session.current_window.clone(),
            session.timeouts.clone(),
            session.frame_context.clone(),
            pressed_keys,
            pressed_buttons,
            pointer_positions,
        )
    };

    let executor = state.get_executor_for_window(&current_window, timeouts, frame_context)?;
    let modifier_state = ModifierState::default();

    for key in pressed_keys {
        executor
            .dispatch_key_event(&key, false, &modifier_state)
            .await?;
    }

    for (source_id, buttons) in pressed_buttons {
        let position = pointer_positions
            .get(&source_id)
            .copied()
            .unwrap_or_default();
        for button in buttons {
            executor
                .dispatch_pointer_event(PointerEventType::Up, position.0, position.1, button, 0)
                .await?;
        }
    }

    Ok(WebDriverResponse::null())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use super::{ActionSequence, ActionsRequest, KeyAction, button_mask, buttons_mask};

    #[test]
    fn webdriver_buttons_map_to_dom_buttons_bitmask() {
        assert_eq!(button_mask(0), 1);
        assert_eq!(button_mask(1), 4);
        assert_eq!(button_mask(2), 2);
        assert_eq!(button_mask(3), 8);
        assert_eq!(button_mask(4), 16);
        assert_eq!(button_mask(5), 0);

        let pressed = HashSet::from([0, 1, 2]);
        assert_eq!(buttons_mask(&pressed), 7);
    }

    #[test]
    fn action_sources_are_transposed_into_ticks_with_max_duration() {
        let request: ActionsRequest = serde_json::from_value(json!({
            "actions": [
                {
                    "type": "key",
                    "id": "keyboard-a",
                    "actions": [
                        { "type": "keyDown", "value": "a" },
                        { "type": "pause", "duration": 25 },
                        { "type": "keyUp", "value": "a" }
                    ]
                },
                {
                    "type": "key",
                    "id": "keyboard-b",
                    "actions": [
                        { "type": "keyDown", "value": "b" },
                        { "type": "pause", "duration": 40 },
                        { "type": "keyUp", "value": "b" }
                    ]
                }
            ]
        }))
        .unwrap();

        assert_eq!(
            request.actions.iter().map(ActionSequence::len).max(),
            Some(3)
        );
        assert_eq!(
            request
                .actions
                .iter()
                .map(|sequence| sequence.duration_at(1))
                .max(),
            Some(40)
        );

        let ActionSequence::Key { actions, .. } = &request.actions[1] else {
            panic!("expected key source");
        };
        assert!(matches!(actions[0], KeyAction::KeyDown { .. }));
        assert!(matches!(actions[2], KeyAction::KeyUp { .. }));
    }
}
