//! Превращение заметок в самостоятельные HTML-документы для просмотра.
//!
//! Каждый документ открывается в изолированном `<iframe sandbox>` по адресу
//! `<base_url><путь заметки>`, поэтому относительные картинки и ссылки
//! работают «как в браузере». Небольшой скрипт-мост (`assets/bridge.js`)
//! перехватывает клики по ссылкам и передаёт их приложению.

use std::collections::HashMap;

use pulldown_cmark::{CowStr, Event, LinkType, Parser, Tag, TagEnd, html};

use super::links::{self, Piece};
use super::paths::{self, escape_html};
use super::vault::{Note, NoteKind, Vault};

const NOTE_CSS: &str = include_str!("../../assets/note.css");
const BRIDGE_JS: &str = include_str!("../../assets/bridge.js");

/// Готовый документ и его Content-Security-Policy.
pub struct Document {
    pub html: String,
    /// `None` — без ограничений (HTML-заметки — это «живые» страницы автора).
    pub csp: Option<String>,
}

/// Контекст рендера одной заметки.
pub struct Renderer<'a> {
    pub vault: &'a Vault,
    /// Базовый URL схемы заметок, например `note://localhost/`.
    pub base_url: &'a str,
    /// Одноразовый nonce для скрипта-моста.
    pub nonce: &'a str,
}

impl Renderer<'_> {
    pub fn render(&self, note: &Note, src: &str) -> Document {
        match note.kind {
            NoteKind::Markdown => self.markdown(note, src),
            NoteKind::Html => self.html(note, src),
        }
    }

    fn url(&self, path: &str, fragment: Option<&str>) -> String {
        let mut url = format!("{}{}", self.base_url, paths::encode_path(path));
        if let Some(f) = fragment {
            url.push('#');
            url.push_str(&paths::slugify(f));
        }
        url
    }

    fn bridge(&self) -> String {
        format!("<script nonce=\"{}\">{}</script>", self.nonce, BRIDGE_JS)
    }

    fn page(&self, title: &str, class: &str, body: &str) -> String {
        format!(
            "<!doctype html><html><head><meta charset=\"utf-8\">\
             <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>{title}</title><style>{NOTE_CSS}</style></head>\
             <body><article class=\"{class}\">{body}</article>{bridge}</body></html>",
            title = escape_html(title),
            bridge = self.bridge(),
        )
    }

    /// `<a>` для разрешённой/неразрешённой wiki-ссылки.
    fn wiki_anchor_open(&self, from: &Note, target: &str, fragment: Option<&str>, embed: bool) -> String {
        let class = if embed { "internal-link embed" } else { "internal-link" };
        if target.is_empty() {
            // `[[#Заголовок]]` — ссылка внутри текущей заметки.
            let frag = fragment.map(paths::slugify).unwrap_or_default();
            return format!("<a class=\"{class}\" href=\"#{}\">", escape_html(&frag));
        }
        match self.vault.resolve_note(target, &from.path) {
            Some(n) => format!(
                "<a class=\"{class}\" href=\"{}\">",
                escape_html(&self.url(&n.path, fragment))
            ),
            None => format!(
                "<a class=\"{class} unresolved\" title=\"Заметка не найдена: {}\">",
                escape_html(target)
            ),
        }
    }

    // -- Markdown ------------------------------------------------------------

    fn markdown(&self, note: &Note, src: &str) -> Document {
        let mut events: Vec<Event> = Parser::new_ext(src, links::markdown_options()).collect();
        assign_heading_ids(&mut events);

        // Для каждого открытого Link/Image: заменили ли мы его на сырой `<a>`.
        let mut replaced: Vec<bool> = Vec::new();
        let events = events.into_iter().map(|ev| match ev {
            Event::Start(Tag::Link { link_type: LinkType::WikiLink { .. }, dest_url, .. }) => {
                replaced.push(true);
                let (target, frag) = paths::split_fragment(&dest_url);
                Event::InlineHtml(self.wiki_anchor_open(note, target, frag, false).into())
            }
            Event::Start(Tag::Image { link_type: LinkType::WikiLink { .. }, dest_url, title, id }) => {
                let (target, frag) = paths::split_fragment(&dest_url);
                if self.vault.resolve_note(target, &note.path).is_some() {
                    // `![[Заметка]]` — показываем как ссылку-вставку.
                    replaced.push(true);
                    return Event::InlineHtml(self.wiki_anchor_open(note, target, frag, true).into());
                }
                replaced.push(false);
                let dest = match self.vault.resolve_file(target, &note.path) {
                    Some(p) => self.url(p, None).into(),
                    None => dest_url,
                };
                Event::Start(Tag::Image { link_type: LinkType::Inline, dest_url: dest, title, id })
            }
            Event::Start(Tag::Link { link_type, dest_url, title, id }) => {
                replaced.push(false);
                let dest_url = self.rewrite_local(note, link_type, dest_url, true);
                Event::Start(Tag::Link { link_type, dest_url, title, id })
            }
            Event::Start(Tag::Image { link_type, dest_url, title, id }) => {
                replaced.push(false);
                let dest_url = self.rewrite_local(note, link_type, dest_url, false);
                Event::Start(Tag::Image { link_type, dest_url, title, id })
            }
            Event::End(end @ (TagEnd::Link | TagEnd::Image)) => {
                if replaced.pop().unwrap_or(false) {
                    Event::InlineHtml("</a>".into())
                } else {
                    Event::End(end)
                }
            }
            other => other,
        });

        let mut body = String::with_capacity(src.len() * 3 / 2);
        html::push_html(&mut body, events);

        // `*` в CSP не покрывает нестандартные схемы, поэтому `note:` указываем явно.
        let own = self.base_url.trim_end_matches('/');
        let csp = format!(
            "default-src 'none'; img-src * {own} data: blob:; media-src * {own}; \
             font-src * {own} data:; style-src * {own} 'unsafe-inline'; frame-src * {own}; \
             script-src 'nonce-{}'",
            self.nonce
        );
        Document { html: self.page(&note.title, "markdown-body", &body), csp: Some(csp) }
    }

    /// Обычные ссылки вида `[x](Идея)` или `![](cat.png)`: если цель есть
    /// в хранилище, подставляем точный URL (Obsidian разрешает их по имени).
    fn rewrite_local<'e>(&self, note: &Note, ty: LinkType, dest: CowStr<'e>, is_link: bool) -> CowStr<'e> {
        if matches!(ty, LinkType::Autolink | LinkType::Email) || paths::is_external(&dest) {
            return dest;
        }
        let (path, frag) = paths::split_fragment(&dest);
        let decoded = paths::decode(path);
        if is_link
            && let Some(n) = self.vault.resolve_note(&decoded, &note.path) {
                return self.url(&n.path, frag).into();
            }
        match self.vault.resolve_file(&decoded, &note.path) {
            Some(p) => self.url(p, None).into(),
            None => dest,
        }
    }

    // -- HTML ----------------------------------------------------------------

    fn html(&self, note: &Note, src: &str) -> Document {
        let mut out = String::with_capacity(src.len() + BRIDGE_JS.len() + 256);
        let mut is_document = false;
        for piece in links::html_pieces(src) {
            match piece {
                Piece::Tag(t) => {
                    if matches!(links::tag_name(t).as_str(), "html" | "body" | "head" | "!doctype") {
                        is_document = true;
                    }
                    out.push_str(t);
                }
                Piece::Raw(r) => out.push_str(r),
                Piece::Text(t) => self.html_text(note, t, &mut out),
            }
        }

        let html = if is_document {
            // Полноценная страница: оставляем её стили, только добавляем мост.
            let pos = out.to_ascii_lowercase().rfind("</body>").unwrap_or(out.len());
            out.insert_str(pos, &self.bridge());
            out
        } else {
            // Фрагмент: оформляем как обычную заметку.
            self.page(&note.title, "markdown-body html-fragment", &out)
        };
        Document { html, csp: None }
    }

    /// Заменяет `[[ссылки]]` в текстовом узле HTML на теги.
    fn html_text(&self, note: &Note, text: &str, out: &mut String) {
        let mut last = 0;
        for w in links::find_wikilinks(text) {
            out.push_str(&text[last..w.start]);
            last = w.end;
            let label = escape_html(&match (w.label, w.fragment) {
                (Some(l), _) => l.to_owned(),
                (None, Some(f)) if w.target.is_empty() => f.to_owned(),
                (None, Some(f)) => format!("{} › {f}", w.target),
                (None, None) => w.target.to_owned(),
            });
            let is_note = self.vault.resolve_note(w.target, &note.path).is_some();
            if w.embed && !is_note
                && let Some(p) = self.vault.resolve_file(w.target, &note.path) {
                    out.push_str(&format!(
                        "<img src=\"{}\" alt=\"{}\">",
                        escape_html(&self.url(p, None)),
                        escape_html(w.label.unwrap_or(w.target))
                    ));
                    continue;
                }
            out.push_str(&self.wiki_anchor_open(note, w.target, w.fragment, w.embed));
            out.push_str(&label);
            out.push_str("</a>");
        }
        out.push_str(&text[last..]);
    }
}

/// Проставляет `id` заголовкам без явного `{#id}`, чтобы работали
/// ссылки `[[Заметка#Заголовок]]`.
fn assign_heading_ids(events: &mut [Event]) {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut i = 0;
    while i < events.len() {
        let needs_id = matches!(&events[i], Event::Start(Tag::Heading { id: None, .. }));
        if !needs_id {
            i += 1;
            continue;
        }
        let mut text = String::new();
        let mut j = i + 1;
        while j < events.len() && !matches!(events[j], Event::End(TagEnd::Heading(_))) {
            if let Event::Text(t) | Event::Code(t) = &events[j] {
                text.push_str(t);
            }
            j += 1;
        }
        let base = paths::slugify(&text);
        if !base.is_empty() {
            let n = seen.entry(base.clone()).or_insert(0);
            let slug = if *n == 0 { base } else { format!("{base}-{n}") };
            *n += 1;
            if let Event::Start(Tag::Heading { id, .. }) = &mut events[i] {
                *id = Some(slug.into());
            }
        }
        i = j;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::vault::tests::sample;

    fn render(path: &str, src: &str) -> Document {
        let vault = sample();
        let note = vault.note(path).unwrap().clone();
        Renderer { vault: &vault, base_url: "note://localhost/", nonce: "n" }.render(&note, src)
    }

    #[test]
    fn markdown_wikilinks_and_headings() {
        let doc = render("index.md", "# План работ\n[[Rust#План работ|тут]] [[Nope]] ![[cat.png]] [p](Page)");
        let h = &doc.html;
        assert!(h.contains(r#"<h1 id="план-работ">"#), "{h}");
        assert!(h.contains(r#"href="note://localhost/Ideas/Rust.md#план-работ">тут</a>"#), "{h}");
        assert!(h.contains(r#"internal-link unresolved"#), "{h}");
        assert!(h.contains(r#"src="note://localhost/img/cat.png""#), "{h}");
        assert!(h.contains(r#"href="note://localhost/Web/Page.html""#), "{h}");
        assert!(doc.csp.unwrap().contains("nonce-n"));
    }

    #[test]
    fn html_document_keeps_structure() {
        let doc = render(
            "Web/Page.html",
            "<!doctype html><html><body><p>[[index]] ![[cat.png]]</p><pre>[[raw]]</pre></body></html>",
        );
        let h = &doc.html;
        assert!(h.contains(r#"href="note://localhost/index.md">index</a>"#), "{h}");
        assert!(h.contains(r#"<img src="note://localhost/img/cat.png""#), "{h}");
        assert!(h.contains("<pre>[[raw]]</pre>"), "{h}");
        assert!(h.contains("<script nonce=\"n\">"), "{h}");
        assert!(!h.contains("markdown-body"));
    }

    #[test]
    fn html_fragment_is_wrapped() {
        let doc = render("Web/Page.html", "<h2>Hi</h2>");
        assert!(doc.html.contains("html-fragment"));
    }
}
