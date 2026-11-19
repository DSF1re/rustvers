//! Состояние приложения: открытое хранилище, его граф и правка заметок.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::core::graph::{self, Graph};
use crate::core::links;
use crate::core::paths;
use crate::core::vault::{NoteKind, Vault};

/// Ссылки заметки, извлечённые при последнем чтении, и время её изменения.
struct Cached {
    modified: Option<SystemTime>,
    targets: Vec<String>,
}

/// Открытое хранилище.
pub struct Session {
    pub vault: Vault,
    pub graph: Graph,
    cache: HashMap<String, Cached>,
}

/// Время изменения файла в миллисекундах (для защиты от перезаписи).
pub fn millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

fn modified(path: &Path) -> Option<SystemTime> {
    fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Session {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        Self::scan(&path.into(), &HashMap::new())
    }

    /// Пересканировать папку; неизменённые файлы не перечитываются.
    pub fn refresh(&self) -> Result<Self, String> {
        Self::scan(self.vault.root(), &self.cache)
    }

    fn scan(root: &Path, old: &HashMap<String, Cached>) -> Result<Self, String> {
        let vault = Vault::open(root).map_err(|e| format!("{}: {e}", root.display()))?;
        let mut cache = HashMap::with_capacity(vault.notes().len());
        for note in vault.notes() {
            let modified = modified(&vault.root().join(&note.path));
            let targets = match old.get(&note.path) {
                Some(c) if modified.is_some() && c.modified == modified => c.targets.clone(),
                _ => vault.read(note).map(|src| links::extract(note.kind, &src)).unwrap_or_default(),
            };
            cache.insert(note.path.clone(), Cached { modified, targets });
        }
        let mut session = Session { vault, graph: Graph::default(), cache };
        session.rebuild_graph();
        Ok(session)
    }

    fn rebuild_graph(&mut self) {
        let cache = &self.cache;
        self.graph = graph::build_from(&self.vault, |n| cache.get(&n.path).map(|c| c.targets.clone()).unwrap_or_default());
    }

    /// Исходный текст заметки и время её изменения.
    pub fn source(&self, path: &str) -> Result<(String, u64), String> {
        let note = self.vault.note(path).ok_or_else(|| format!("Нет заметки {path}"))?;
        let full = self.vault.root().join(&note.path);
        let modified = modified(&full).map_or(0, millis);
        let text = self.vault.read(note).map_err(|e| e.to_string())?;
        Ok((text, modified))
    }

    /// Сохраняет заметку и обновляет граф (без обхода папки).
    ///
    /// `expected` — время изменения, с которым файл был открыт в редакторе:
    /// если файл с тех пор изменили снаружи, сохранение отклоняется.
    pub fn save(&mut self, path: &str, content: &str, expected: Option<u64>) -> Result<u64, String> {
        let note = self.vault.note(path).ok_or_else(|| format!("Нет заметки {path}"))?.clone();
        let full = self.vault.root().join(&note.path);
        if let (Some(expected), Some(now)) = (expected, modified(&full))
            && millis(now) != expected {
                return Err("Файл изменён вне редактора. Пересканируйте хранилище (Ctrl+R), чтобы загрузить новую версию.".into());
            }
        write_atomic(&full, content.as_bytes()).map_err(|e| format!("Не удалось сохранить: {e}"))?;

        let modified = modified(&full);
        self.cache.insert(
            note.path.clone(),
            Cached { modified, targets: links::extract(note.kind, content) },
        );
        self.rebuild_graph();
        Ok(modified.map_or(0, millis))
    }

    /// Создаёт заметку по вводу пользователя (`Идея`, `папка/Идея.html`).
    /// Возвращает обновлённую сессию и путь новой заметки.
    pub fn create(&self, spec: &str) -> Result<(Self, String), String> {
        let path = self.vault.new_note_path(spec)?;
        let full = self.vault.root().join(&path);
        if let Some(dir) = full.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let title = paths::file_stem(&path);
        let body = match NoteKind::from_path(&path) {
            Some(NoteKind::Html) => html_template(title),
            _ => format!("# {title}\n\n"),
        };
        // create_new: не затираем файл, если он появился между проверкой и записью.
        fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)
            .and_then(|mut f| f.write_all(body.as_bytes()))
            .map_err(|e| format!("Не удалось создать {path}: {e}"))?;
        Ok((self.refresh()?, path))
    }

    /// Переименовывает или переносит заметку и поправляет `[[ссылки]]` на неё
    /// в остальных заметках. Возвращает обновлённую сессию и новый путь.
    pub fn rename(&self, old: &str, spec: &str) -> Result<(Self, String), String> {
        let new = self.vault.rename_path(old, spec)?;
        if new == old {
            return Ok((self.refresh()?, new));
        }
        let root = self.vault.root();
        let (from, to) = (root.join(old), root.join(&new));
        let case_only = new.to_lowercase() == old.to_lowercase();
        if to.exists() && !case_only {
            return Err(format!("«{new}» уже существует"));
        }
        if let Some(dir) = to.parent() {
            fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        fs::rename(&from, &to).map_err(|e| format!("Не удалось переименовать: {e}"))?;
        let mut fresh = self.refresh()?;

        // Ссылки правим по старому индексу, а новый текст подбираем по новому.
        let mut failed = 0;
        let notes: Vec<_> = self.vault.notes().to_vec();
        for note in &notes {
            let was = &note.path;
            let now = if was == old { new.as_str() } else { was.as_str() };
            let full = fresh.vault.root().join(now);
            let Ok(src) = fs::read_to_string(&full) else { continue };
            let Some(text) = rewrite_links(note.kind, &src, |target| {
                let hit = self.vault.resolve_note(target, was)?;
                if hit.path != old {
                    return None;
                }
                Some(link_text(&fresh.vault, target, now, &new))
            }) else { continue };
            if write_atomic(&full, text.as_bytes()).is_err() {
                failed += 1;
            }
        }
        fresh = fresh.refresh()?;
        if failed > 0 {
            return Err(format!("Заметка переименована, но ссылки не обновлены в {failed} файлах"));
        }
        Ok((fresh, new))
    }

    /// Удаляет заметку (файл) и возвращает обновлённую сессию.
    pub fn delete(&self, path: &str) -> Result<Self, String> {
        let note = self.vault.note(path).ok_or_else(|| format!("Нет заметки {path}"))?;
        fs::remove_file(self.vault.root().join(&note.path)).map_err(|e| format!("Не удалось удалить: {e}"))?;
        self.refresh()
    }

    pub fn info(&self) -> VaultInfo<'_> {
        VaultInfo {
            name: self.vault.name(),
            root: self.vault.root().display().to_string(),
            base_url: super::protocol::BASE_URL,
            graph: &self.graph,
        }
    }
}

/// Запись через временный файл + rename: при сбое старая версия цела.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Имя с точкой — сканер хранилища такие файлы пропускает.
    let tmp = path.with_file_name(format!(".{name}.rustvers-tmp"));
    let result = (|| {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Заменяет цели `[[вики-ссылок]]` в Markdown/HTML; `None`, если менять нечего.
fn rewrite_links(kind: NoteKind, src: &str, mut f: impl FnMut(&str) -> Option<String>) -> Option<String> {
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    let mut scan = |text: &str, base: usize| {
        for w in links::find_wikilinks(text) {
            if w.embed {
                continue; // `![[...]]` — вложения, не заметки
            }
            if let Some(new) = f(w.target) {
                // `target` — срез `text`, поэтому смещение берём из указателей.
                let start = base + (w.target.as_ptr() as usize - text.as_ptr() as usize);
                spans.push((start, start + w.target.len(), new));
            }
        }
    };
    match kind {
        NoteKind::Markdown => scan(src, 0),
        NoteKind::Html => {
            for piece in links::html_pieces(src) {
                if let links::Piece::Text(t) = piece {
                    scan(t, t.as_ptr() as usize - src.as_ptr() as usize);
                }
            }
        }
    }
    if spans.is_empty() {
        return None;
    }
    spans.sort_by_key(|s| s.0);
    let mut out = String::with_capacity(src.len() + 32);
    let mut pos = 0;
    for (a, b, new) in spans {
        out.push_str(&src[pos..a]);
        out.push_str(&new);
        pos = b;
    }
    out.push_str(&src[pos..]);
    Some(out)
}

/// Текст ссылки на заметку `new` из заметки `from`: короткое имя, если оно
/// однозначно, иначе путь. Расширение сохраняется, если оно было в ссылке.
fn link_text(vault: &Vault, old_target: &str, from: &str, new: &str) -> String {
    let with_ext = paths::extension(old_target).is_some_and(|e| ["md", "html", "htm", "markdown"].contains(&e.as_str()));
    let (stem, name) = (paths::file_stem(new), paths::file_name(new));
    let strip = |p: &str| if with_ext { p.to_owned() } else { p.rsplit_once('.').map_or(p, |(s, _)| s).to_owned() };
    for cand in [if with_ext { name } else { stem }.to_owned(), strip(new)] {
        if vault.resolve_note(&cand, from).is_some_and(|n| n.path == new) {
            return cand;
        }
    }
    strip(new)
}

fn html_template(title: &str) -> String {
    let t = paths::escape_html(title);
    format!(
        "<!doctype html>\n<html lang=\"ru\">\n<head>\n<meta charset=\"utf-8\">\n<title>{t}</title>\n<style>\n  body {{ font-family: system-ui, sans-serif; max-width: 760px; margin: 40px auto; padding: 0 24px; }}\n</style>\n</head>\n<body>\n<h1>{t}</h1>\n\n<p></p>\n</body>\n</html>\n"
    )
}

#[derive(Default)]
pub struct AppState {
    pub session: RwLock<Option<Session>>,
    /// Хранилище, которое нужно открыть при старте (аргумент или последнее).
    pub initial: Mutex<Option<PathBuf>>,
}

/// То, что получает фронтенд.
#[derive(Serialize)]
pub struct VaultInfo<'a> {
    pub name: String,
    pub root: String,
    pub base_url: &'static str,
    #[serde(flatten)]
    pub graph: &'a Graph,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_vault(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rustvers-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.md"), "# A\n").unwrap();
        fs::write(dir.join("b.md"), "# B\n").unwrap();
        dir
    }

    #[test]
    fn save_updates_graph_and_detects_conflicts() {
        let dir = temp_vault("save");
        let mut s = Session::open(&dir).unwrap();
        assert!(s.graph.edges.is_empty());

        let (_, mtime) = s.source("a.md").unwrap();
        let new = s.save("a.md", "# A\n[[b]]", Some(mtime)).unwrap();
        assert_eq!(s.graph.edges, vec![[0, 1]]);
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "# A\n[[b]]");
        assert!(!dir.join(".a.md.rustvers-tmp").exists());

        // Устаревшее время изменения — отказ, файл не тронут.
        assert!(s.save("a.md", "затёрто", Some(new.wrapping_sub(5000))).is_err());
        assert_eq!(fs::read_to_string(dir.join("a.md")).unwrap(), "# A\n[[b]]");
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rename_updates_links() {
        let dir = temp_vault("rename");
        fs::write(dir.join("a.md"), "см. [[b]], [[b#h|метка]], ![[b.png]] и [[b.md]]\n").unwrap();
        fs::write(dir.join("c.html"), "<p>[[b]]</p><script>x = '[[b]]'</script>").unwrap();
        let s = Session::open(&dir).unwrap();
        let (s, path) = s.rename("b.md", "Новая").unwrap();
        assert_eq!(path, "Новая.md");
        assert!(!dir.join("b.md").exists() && dir.join("Новая.md").exists());
        assert_eq!(
            fs::read_to_string(dir.join("a.md")).unwrap(),
            "см. [[Новая]], [[Новая#h|метка]], ![[b.png]] и [[Новая.md]]\n"
        );
        assert_eq!(fs::read_to_string(dir.join("c.html")).unwrap(), "<p>[[Новая]]</p><script>x = '[[b]]'</script>");
        assert!(s.graph.nodes.iter().all(|n| n.id != "b.md"));

        // Перенос в папку и конфликт имён.
        let (s, path) = s.rename("Новая.md", "папка/Тема").unwrap();
        assert_eq!(path, "папка/Тема.md");
        assert!(fs::read_to_string(dir.join("a.md")).unwrap().contains("[[Тема]]"));
        assert!(s.rename("a.md", "папка/Тема").is_err());

        let s = s.delete("a.md").unwrap();
        assert!(!dir.join("a.md").exists() && s.vault.note("a.md").is_none());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn create_note_and_refresh() {
        let dir = temp_vault("create");
        let s = Session::open(&dir).unwrap();
        let (s, path) = s.create("папка/Новая").unwrap();
        assert_eq!(path, "папка/Новая.md");
        assert!(s.vault.note("папка/Новая.md").is_some());
        assert!(s.create("a").is_err());
        let (s, path) = s.create("Страница.html").unwrap();
        assert!(fs::read_to_string(dir.join(&path)).unwrap().contains("<h1>Страница</h1>"));
        assert_eq!(s.graph.nodes.len(), 4);
        let _ = fs::remove_dir_all(dir);
    }
}
