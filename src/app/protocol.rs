//! Схема `note://` — отдаёт отрендеренные заметки и файлы хранилища.
//!
//! `note://localhost/папка/Заметка.md` → HTML-документ заметки,
//! `note://localhost/img/cat.png` → сам файл.

use std::borrow::Cow;
use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::{SystemTime, UNIX_EPOCH};

use tauri::http::{Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager, Runtime, UriSchemeContext, UriSchemeResponder};

use super::state::AppState;
use crate::core::paths;
use crate::core::render::Renderer;

pub const SCHEME: &str = "note";

/// Адрес схемы в WebKitGTK.
pub const BASE_URL: &str = "note://localhost/";

type Resp = Response<Cow<'static, [u8]>>;

pub fn handle<R: Runtime>(ctx: UriSchemeContext<'_, R>, req: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let app = ctx.app_handle().clone();
    // Чтение и рендер — не в главном потоке, чтобы не подвешивать UI.
    std::thread::spawn(move || responder.respond(serve(&app, req.uri().path())));
}

fn serve<R: Runtime>(app: &AppHandle<R>, raw_path: &str) -> Resp {
    let path = paths::decode(raw_path.trim_start_matches('/'));
    let state = app.state::<AppState>();
    let guard = state.session.read().unwrap_or_else(|e| e.into_inner());
    let Some(session) = guard.as_ref() else {
        return error(StatusCode::SERVICE_UNAVAILABLE, "Хранилище не открыто");
    };
    let vault = &session.vault;

    if let Some(note) = vault.note(&path) {
        let src = match vault.read(note) {
            Ok(s) => s,
            Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        };
        let nonce = nonce();
        let doc = Renderer { vault, base_url: BASE_URL, nonce: &nonce }.render(note, &src);
        let mut resp = Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .header(header::CACHE_CONTROL, "no-store");
        if let Some(csp) = doc.csp {
            resp = resp.header(header::CONTENT_SECURITY_POLICY, csp);
        }
        return resp.body(Cow::Owned(doc.html.into_bytes())).unwrap();
    }

    match vault.file_path(&path).map(std::fs::read) {
        Some(Ok(bytes)) => Response::builder()
            .header(header::CONTENT_TYPE, mime(&path))
            .body(Cow::Owned(bytes))
            .unwrap(),
        Some(Err(e)) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        None => error(StatusCode::NOT_FOUND, &format!("Не найдено: {path}")),
    }
}

fn error(status: StatusCode, msg: &str) -> Resp {
    Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(Cow::Owned(msg.as_bytes().to_vec()))
        .unwrap()
}

/// Непредсказуемая строка для CSP-nonce (без внешних зависимостей).
fn nonce() -> String {
    let mut h = RandomState::new().build_hasher();
    h.write_u128(SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    format!("{:016x}{:016x}", h.finish(), RandomState::new().build_hasher().finish())
}

fn mime(path: &str) -> &'static str {
    match paths::extension(path).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        Some("ico") => "image/x-icon",
        Some("bmp") => "image/bmp",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("pdf") => "application/pdf",
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json",
        Some("txt") => "text/plain; charset=utf-8",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("ttf") => "font/ttf",
        _ => "application/octet-stream",
    }
}
