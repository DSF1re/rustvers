//! Команды нативного графа (см. `native/`).

use std::collections::HashMap;
use std::sync::Mutex;

use serde::Deserialize;
use tauri::{AppHandle, Runtime, State, WebviewWindow};

use super::native::{NativeGraph, Options, Rect};
use super::state::AppState;

/// Нативный граф; `None`, пока не запущен.
#[derive(Default)]
pub struct NativeSlot(Mutex<Option<NativeGraph>>);

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GraphMsg {
    /// Взять граф из открытого хранилища. `fresh` — заново вписать в экран.
    Data { fresh: bool },
    Layout { rect: Option<Rect> },
    Options { show_missing: bool, show_orphans: bool, labels: bool },
    Selected { id: Option<String> },
    Query { text: String },
    Theme { colors: HashMap<String, String> },
    Fit,
    Fps { on: bool },
}

/// Запускает нативный граф. Синхронная команда — выполняется в главном
/// потоке, где доступны виджеты GTK.
#[tauri::command]
pub fn graph_native_init<R: Runtime>(
    app: AppHandle<R>,
    window: WebviewWindow<R>,
    slot: State<'_, NativeSlot>,
) -> Result<(), String> {
    let mut g = slot.0.lock().unwrap();
    if g.is_none() {
        *g = Some(NativeGraph::start(&app, &window)?);
    }
    Ok(())
}

#[tauri::command]
pub fn graph_native(msg: GraphMsg, slot: State<'_, NativeSlot>, state: State<'_, AppState>) {
    let g = slot.0.lock().unwrap();
    let Some(native) = g.as_ref() else { return };
    match msg {
        GraphMsg::Data { fresh } => {
            if let Some(s) = state.session.read().unwrap().as_ref() {
                native.set_data(s.graph.clone(), fresh);
            }
        }
        GraphMsg::Layout { rect } => native.set_layout(rect),
        GraphMsg::Options { show_missing, show_orphans, labels } => {
            native.set_options(Options { show_missing, show_orphans, labels })
        }
        GraphMsg::Selected { id } => native.set_selected(id),
        GraphMsg::Query { text } => native.set_query(text),
        GraphMsg::Theme { colors } => native.set_theme(colors),
        GraphMsg::Fit => native.fit(),
        GraphMsg::Fps { on } => native.show_fps(on),
    }
}
