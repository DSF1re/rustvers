//! Rustvers — лёгкое хранилище заметок (Markdown + HTML) с графом связей.
//!
//! * [`core`] — чистая логика: индекс файлов, ссылки, рендер, граф.
//! * `app` — оболочка на Tauri (окно, команды, схема `note://`).

pub mod core;
mod app;

pub use app::run;
