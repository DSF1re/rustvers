//! Rustvers — лёгкое хранилище заметок (Markdown + HTML) с графом связей для Linux.
//!
//! * [`core`] — чистая логика: индекс файлов, ссылки, рендер, граф.
//! * `app` — оболочка на Tauri (окно, команды, схема `note://`).

#[cfg(not(target_os = "linux"))]
compile_error!("Rustvers поддерживает только Linux (WebKitGTK + Wayland).");

pub mod core;
mod app;

pub use app::run;
