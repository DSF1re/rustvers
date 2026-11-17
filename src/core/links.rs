//! Извлечение ссылок из заметок и разбор `[[wiki-ссылок]]`.

use pulldown_cmark::{Event, LinkType, Options, Parser, Tag};

use super::paths;
use super::vault::NoteKind;

/// Опции Markdown, общие для разбора ссылок и рендера.
pub fn markdown_options() -> Options {
    Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_HEADING_ATTRIBUTES
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS
        | Options::ENABLE_GFM
        | Options::ENABLE_DEFINITION_LIST
        | Options::ENABLE_WIKILINKS
}

/// Все исходящие ссылки заметки (цели без `#фрагмента`, ещё не разрешённые).
pub fn extract(kind: NoteKind, src: &str) -> Vec<String> {
    let mut out = match kind {
        NoteKind::Markdown => extract_markdown(src),
        NoteKind::Html => extract_html(src),
    };
    out.retain(|t| !t.trim().is_empty());
    out
}

fn extract_markdown(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for event in Parser::new_ext(src, markdown_options()) {
        let (link_type, dest) = match event {
            Event::Start(Tag::Link { link_type, dest_url, .. }) => (link_type, dest_url),
            Event::Start(Tag::Image { link_type, dest_url, .. }) => (link_type, dest_url),
            _ => continue,
        };
        match link_type {
            LinkType::WikiLink { .. } => out.push(paths::split_fragment(&dest).0.to_owned()),
            LinkType::Autolink | LinkType::Email => {}
            _ if paths::is_external(&dest) => {}
            _ => out.push(paths::decode(paths::split_fragment(&dest).0)),
        }
    }
    out
}

fn extract_html(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for piece in html_pieces(src) {
        match piece {
            Piece::Tag(tag) if tag_name(tag) == "a" => {
                if let Some(href) = attr(tag, "href")
                    && !paths::is_external(&href) {
                        out.push(paths::decode(paths::split_fragment(&href).0));
                    }
            }
            Piece::Text(text) => {
                out.extend(find_wikilinks(text).into_iter().map(|w| w.target.to_owned()));
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Wiki-ссылки
// ---------------------------------------------------------------------------

/// `[[цель#фрагмент|подпись]]` или `![[вложение]]`, найденная в тексте.
#[derive(Debug, PartialEq, Eq)]
pub struct WikiLink<'a> {
    /// Байтовый диапазон в исходном тексте (включая `!` для вставок).
    pub start: usize,
    pub end: usize,
    pub embed: bool,
    pub target: &'a str,
    pub fragment: Option<&'a str>,
    pub label: Option<&'a str>,
}

/// Находит wiki-ссылки в обычном тексте (используется для HTML-заметок;
/// Markdown разбирается самим pulldown-cmark).
pub fn find_wikilinks(text: &str) -> Vec<WikiLink<'_>> {
    let mut out = Vec::new();
    let mut pos = 0;
    while let Some(off) = text[pos..].find("[[") {
        let open = pos + off;
        let inner_start = open + 2;
        let Some(len) = text[inner_start..].find("]]") else { break };
        let inner = &text[inner_start..inner_start + len];
        let end = inner_start + len + 2;

        if inner.is_empty() || inner.contains('\n') || inner.contains("[[") {
            pos = inner_start;
            continue;
        }
        let (dest, label) = match inner.split_once('|') {
            Some((d, l)) => (d, Some(l.trim()).filter(|l| !l.is_empty())),
            None => (inner, None),
        };
        let (target, fragment) = paths::split_fragment(dest.trim());
        let embed = open > 0 && text.as_bytes()[open - 1] == b'!';
        out.push(WikiLink {
            start: if embed { open - 1 } else { open },
            end,
            embed,
            target: target.trim(),
            fragment,
            label,
        });
        pos = end;
    }
    out
}

// ---------------------------------------------------------------------------
// Минимальный токенизатор HTML
// ---------------------------------------------------------------------------

/// Кусок HTML-документа.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece<'a> {
    /// Тег целиком: `<a href="...">`, `</div>`, `<!doctype html>`.
    Tag(&'a str),
    /// Содержимое, которое нельзя трогать: комментарии, `<script>`, `<style>`, `<pre>`, `<code>`…
    Raw(&'a str),
    /// Обычный текст между тегами.
    Text(&'a str),
}

/// Элементы, внутри которых текст не обрабатывается.
const RAW_ELEMENTS: [&str; 6] = ["script", "style", "textarea", "pre", "code", "title"];

/// Делит HTML на теги, «сырые» блоки и текст. Не полноценный парсер,
/// но корректно обходит кавычки в атрибутах, комментарии и `<script>`.
pub fn html_pieces<'a>(src: &'a str) -> Vec<Piece<'a>> {
    let lower = src.to_ascii_lowercase();
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    let mut text_start = 0;

    let flush = |out: &mut Vec<Piece<'a>>, from: usize, to: usize| {
        if to > from {
            out.push(Piece::Text(&src[from..to]));
        }
    };

    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        if lower[i..].starts_with("<!--") {
            flush(&mut out, text_start, i);
            let end = lower[i + 4..].find("-->").map_or(src.len(), |e| i + 4 + e + 3);
            out.push(Piece::Raw(&src[i..end]));
            i = end;
            text_start = i;
            continue;
        }
        let starts_tag = bytes
            .get(i + 1)
            .is_some_and(|&c| c.is_ascii_alphabetic() || matches!(c, b'/' | b'!' | b'?'));
        if !starts_tag {
            i += 1;
            continue;
        }

        flush(&mut out, text_start, i);
        let end = tag_end(bytes, i);
        let tag = &src[i..end];
        out.push(Piece::Tag(tag));
        i = end;

        let name = tag_name(tag);
        if !tag.starts_with("</") && RAW_ELEMENTS.contains(&name.as_str()) && !tag.ends_with("/>") {
            let close = format!("</{name}");
            let raw_end = lower[i..].find(&close).map_or(src.len(), |e| i + e);
            if raw_end > i {
                out.push(Piece::Raw(&src[i..raw_end]));
            }
            i = raw_end;
        }
        text_start = i;
    }
    flush(&mut out, text_start, src.len());
    out
}

/// Позиция сразу после `>` с учётом кавычек в атрибутах.
fn tag_end(bytes: &[u8], start: usize) -> usize {
    let mut quote = None;
    for (j, &c) in bytes.iter().enumerate().skip(start + 1) {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(c),
            (None, b'>') => return j + 1,
            _ => {}
        }
    }
    bytes.len()
}

/// Имя тега в нижнем регистре: `<A HREF=..>` → `a`, `</div>` → `div`.
pub fn tag_name(tag: &str) -> String {
    tag.trim_start_matches('<')
        .trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Значение атрибута тега (без декодирования HTML-сущностей, кроме `&amp;`).
pub fn attr(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let bytes = tag.as_bytes();
    let mut from = 0;
    while let Some(off) = lower[from..].find(name) {
        let at = from + off;
        from = at + name.len();
        let before_ok = at > 0 && bytes[at - 1].is_ascii_whitespace();
        let rest = tag[from..].trim_start();
        if !before_ok || !rest.starts_with('=') {
            continue;
        }
        let value = rest[1..].trim_start();
        let parsed = match value.chars().next()? {
            q @ ('"' | '\'') => value[1..].split(q).next()?,
            _ => value.split(|c: char| c.is_whitespace() || c == '>').next()?,
        };
        return Some(parsed.replace("&amp;", "&"));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wikilinks_in_text() {
        let links = find_wikilinks("см. [[Идея#План|план]] и ![[cat.png]], но не [[]]");
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].target, "Идея");
        assert_eq!(links[0].fragment, Some("План"));
        assert_eq!(links[0].label, Some("план"));
        assert!(links[1].embed);
        assert_eq!(links[1].target, "cat.png");
    }

    #[test]
    fn markdown_links() {
        let src = "---\ntitle: x\n---\n# T\n[[A]] [b](B.md#x) [ext](https://x.org) ![i](img/c.png)\n`[[code]]`";
        assert_eq!(extract(NoteKind::Markdown, src), vec!["A", "B.md", "img/c.png"]);
    }

    #[test]
    fn html_links() {
        let src = r#"<p title="a > b">[[One]] <a class=x href='Two.md#y'>t</a></p>
            <script>let s = "[[NotALink]] <a href='no'>";</script>
            <!-- [[Comment]] --><a href="https://e.org">e</a><code>[[Code]]</code>"#;
        assert_eq!(extract(NoteKind::Html, src), vec!["One", "Two.md"]);
    }

    #[test]
    fn attributes() {
        assert_eq!(attr(r#"<a data-href="x" href="y&amp;z">"#, "href").as_deref(), Some("y&z"));
        assert_eq!(attr("<a href=plain>", "href").as_deref(), Some("plain"));
        assert_eq!(tag_name("</DIV >"), "div");
    }
}
