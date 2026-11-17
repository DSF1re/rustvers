//! GUI-оболочка на Tauri: окно, команды и схема `note://`.

mod commands;
mod protocol;
#[cfg(target_os = "linux")]
mod refresh_rate;
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
        ])
        .setup(|app| {
            let initial = commands::initial_vault(app.handle());
            *app.state::<state::AppState>().initial.lock().unwrap() = initial;

            #[cfg(target_os = "linux")]
            if let Some(window) = app.get_webview_window("main") {
                // Рисовать на частоте монитора, а не на 60 fps (см. refresh_rate.rs).
                // Перезагрузка — чтобы настройка точно применилась к уже открытой странице.
                let _ = window.with_webview(|wv| {
                    let view = wv.inner();
                    if refresh_rate::unlock(&view) {
                        webkit2gtk::WebViewExt::reload(&view);
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("не удалось запустить приложение");
}
