//! Небольшие утилиты для работы с путями внутри хранилища.
//!
//! Внутри приложения путь заметки — это относительный путь от корня
//! хранилища с разделителем `/` (например, `projects/idea.md`).

use percent_encoding::{AsciiSet, CONTROLS, percent_decode_str, utf8_percent_encode};

/// Символы, которые кодируются в сегменте URL-пути.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}')
    .add(b'\\')
    .add(b'^')
    .add(b'|')
    .add(b'[')
    .add(b']');

/// Ссылка ведёт наружу (URL со схемой) или на якорь текущей страницы.
pub fn is_external(link: &str) -> bool {
    let l = link.trim_start();
    if l.starts_with('#') || l.starts_with("//") {
        return true;
    }
    // Схема: буквы/цифры/+-. до первого ':' (Windows-диски вида C: тоже считаем внешними).
    match l.find(':') {
        Some(i) if i > 0 => l[..i]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')),
        _ => false,
    }
}

/// Директория, в которой лежит путь (`a/b/c.md` → `a/b`, `c.md` → ``).
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(dir, _)| dir)
}

/// Имя файла без директории.
pub fn file_name(path: &str) -> &str {
    path.rsplit_once('/').map_or(path, |(_, name)| name)
}

/// Имя файла без расширения.
pub fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    match name.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => name,
    }
}

/// Расширение в нижнем регистре.
pub fn extension(path: &str) -> Option<String> {
    let name = file_name(path);
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => Some(ext.to_ascii_lowercase()),
        _ => None,
    }
}

/// Склеивает `base` и `rel` и нормализует `.`/`..`.
/// Возвращает `None`, если путь выходит за корень хранилища.
pub fn join_normalize(base: &str, rel: &str) -> Option<String> {
    let rel = rel.replace('\\', "/");
    let mut parts: Vec<&str> = Vec::new();
    let start = if rel.starts_with('/') { "" } else { base };
    for seg in start.split('/').chain(rel.split('/')) {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    Some(parts.join("/"))
}

/// Делит строку по первому `#` на путь и фрагмент.
pub fn split_fragment(s: &str) -> (&str, Option<&str>) {
    match s.split_once('#') {
        Some((p, f)) => (p, Some(f).filter(|f| !f.is_empty())),
        None => (s, None),
    }
}

/// Процент-декодирование ссылки из Markdown/HTML.
pub fn decode(s: &str) -> String {
    percent_decode_str(s).decode_utf8_lossy().into_owned()
}

/// Кодирует относительный путь для использования в URL (по сегментам).
pub fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|seg| utf8_percent_encode(seg, SEGMENT).to_string())
        .collect::<Vec<_>>()
        .join("/")
}

/// Якорь для заголовка: `Мой Заголовок!` → `мой-заголовок`.
/// Используется и для `id` заголовков, и для `[[Заметка#Заголовок]]`,
/// поэтому обе стороны всегда совпадают.
pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut dash = false;
    for c in text.chars() {
        if c.is_alphanumeric() {
            out.extend(c.to_lowercase());
            dash = false;
        } else if (c.is_whitespace() || c == '-' || c == '_') && !out.is_empty() && !dash {
            out.push('-');
            dash = true;
        }
    }
    if out.ends_with('-') {
        out.pop();
    }
    out
}

/// Экранирование текста для HTML (в том числе внутри атрибутов).
pub fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_detection() {
        assert!(is_external("https://x.org"));
        assert!(is_external("mailto:a@b.c"));
        assert!(is_external("#top"));
        assert!(!is_external("notes/a.md"));
        assert!(!is_external("Моя заметка"));
    }

    #[test]
    fn normalize() {
        assert_eq!(join_normalize("a/b", "../c.md").as_deref(), Some("a/c.md"));
        assert_eq!(join_normalize("a", "/x/y").as_deref(), Some("x/y"));
        assert_eq!(join_normalize("", "../x"), None);
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("Мой  Заголовок!"), "мой-заголовок");
        assert_eq!(slugify("Hello, World_2"), "hello-world-2");
    }

    #[test]
    fn stems() {
        assert_eq!(file_stem("a/b/note.md"), "note");
        assert_eq!(file_stem(".hidden"), ".hidden");
        assert_eq!(extension("a/B.HTML").as_deref(), Some("html"));
    }
}
