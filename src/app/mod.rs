//! GUI-оболочка на Tauri: окно, команды и схема `note://`.

mod commands;
mod protocol;
mod state;

use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(state::AppState::default())
        .register_asynchronous_uri_scheme_protocol(protocol::SCHEME, protocol::handle)
        .invoke_handler(tauri::generate_handler![
            commands::open_vault,
            commands::current_vault,
            commands::rescan,
            commands::note_source,
            commands::save_note,
            commands::create_note,
            commands::rename_note,
            commands::delete_note,
        ])
        .setup(|app| {
            let initial = commands::initial_vault(app.handle());
            *app.state::<state::AppState>().initial.lock().unwrap() = initial;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("не удалось запустить приложение");
}
