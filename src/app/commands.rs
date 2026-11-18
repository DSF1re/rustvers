//! Команды, доступные фронтенду через `invoke`.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime, State};

use super::state::{AppState, Session};

const LAST_VAULT_FILE: &str = "last_vault";

/// Открывает хранилище и возвращает граф. Запоминает выбор.
#[tauri::command]
pub async fn open_vault<R: Runtime>(
    app: AppHandle<R>,
    state: State<'_, AppState>,
    path: String,
) -> Result<serde_json::Value, String> {
    let session = Session::open(&path)?;
    remember(&app, session.vault.root());
    install(&state, session)
}

/// Текущее хранилище; при первом вызове открывает стартовое.
#[tauri::command]
pub async fn current_vault(state: State<'_, AppState>) -> Result<Option<serde_json::Value>, String> {
    if let Some(s) = state.session.read().unwrap().as_ref() {
        return to_json(s).map(Some);
    }
    let initial = state.initial.lock().unwrap().take();
    match initial {
        Some(path) => install(&state, Session::open(path)?).map(Some),
        None => Ok(None),
    }
}

/// Пересканировать папку (после правок заметок снаружи).
/// Неизменённые файлы не перечитываются.
#[tauri::command]
pub async fn rescan(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let fresh = state.session.read().unwrap().as_ref().ok_or(NOT_OPEN)?.refresh()?;
    install(&state, fresh)
}

const NOT_OPEN: &str = "Хранилище не открыто";

#[derive(Serialize)]
pub struct NoteSource {
    content: String,
    /// Время изменения файла (мс) — вернуть в save_note.
    mtime: u64,
}

/// Исходный текст заметки для редактора.
#[tauri::command]
pub async fn note_source(state: State<'_, AppState>, path: String) -> Result<NoteSource, String> {
    let guard = state.session.read().unwrap();
    let (content, mtime) = guard.as_ref().ok_or(NOT_OPEN)?.source(&path)?;
    Ok(NoteSource { content, mtime })
}

#[derive(Serialize)]
pub struct Saved {
    mtime: u64,
    vault: serde_json::Value,
}

/// Сохраняет заметку. Отказывает, если файл изменили снаружи после `mtime`.
#[tauri::command]
pub async fn save_note(
    state: State<'_, AppState>,
    path: String,
    content: String,
    mtime: Option<u64>,
) -> Result<Saved, String> {
    let mut guard = state.session.write().unwrap();
    let session = guard.as_mut().ok_or(NOT_OPEN)?;
    let mtime = session.save(&path, &content, mtime)?;
    Ok(Saved { mtime, vault: to_json(session)? })
}

#[derive(Serialize)]
pub struct Created {
    path: String,
    vault: serde_json::Value,
}

/// Создаёт заметку: `Имя`, `папка/Имя` или `Имя.html`.
#[tauri::command]
pub async fn create_note(state: State<'_, AppState>, spec: String) -> Result<Created, String> {
    let (session, path) = state.session.read().unwrap().as_ref().ok_or(NOT_OPEN)?.create(&spec)?;
    let vault = install(&state, session)?;
    Ok(Created { path, vault })
}

fn install(state: &AppState, session: Session) -> Result<serde_json::Value, String> {
    let json = to_json(&session)?;
    *state.session.write().unwrap() = Some(session);
    Ok(json)
}

fn to_json(session: &Session) -> Result<serde_json::Value, String> {
    serde_json::to_value(session.info()).map_err(|e| e.to_string())
}

/// Хранилище для старта: аргумент командной строки или последнее открытое.
pub fn initial_vault<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    if let Some(arg) = std::env::args_os().nth(1) {
        return Some(PathBuf::from(arg));
    }
    let file = app.path().app_config_dir().ok()?.join(LAST_VAULT_FILE);
    let path = PathBuf::from(std::fs::read_to_string(file).ok()?.trim());
    path.is_dir().then_some(path)
}

fn remember<R: Runtime>(app: &AppHandle<R>, root: &std::path::Path) {
    let Ok(dir) = app.path().app_config_dir() else { return };
    let _ = std::fs::create_dir_all(&dir);
    let _ = std::fs::write(dir.join(LAST_VAULT_FILE), root.to_string_lossy().as_bytes());
}
