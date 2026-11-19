//! Хранилище: индекс файлов на диске и разрешение ссылок.

use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use super::paths;

/// Тип заметки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NoteKind {
    Markdown,
    Html,
}

impl NoteKind {
    pub fn from_path(path: &str) -> Option<Self> {
        match paths::extension(path)?.as_str() {
            "md" | "markdown" => Some(Self::Markdown),
            "html" | "htm" => Some(Self::Html),
            _ => None,
        }
    }
}

/// Расширения, которые можно опускать в ссылках.
const NOTE_EXTENSIONS: [&str; 4] = ["md", "html", "htm", "markdown"];

#[derive(Debug, Clone)]
pub struct Note {
    /// Относительный путь с разделителем `/`.
    pub path: String,
    pub title: String,
    pub kind: NoteKind,
}

/// Снимок содержимого папки хранилища.
pub struct Vault {
    root: PathBuf,
    notes: Vec<Note>,
    /// путь в нижнем регистре → индекс заметки
    note_by_path: HashMap<String, usize>,
    /// имя без расширения в нижнем регистре → индексы заметок
    note_by_stem: HashMap<String, Vec<usize>>,
    /// путь в нижнем регистре → настоящий путь (все файлы, включая вложения)
    file_by_path: HashMap<String, String>,
    /// имя файла в нижнем регистре → настоящие пути
    file_by_name: HashMap<String, Vec<String>>,
}

impl Vault {
    /// Сканирует папку. Скрытые файлы и папки (`.git`, `.obsidian`, …) пропускаются.
    pub fn open(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = dunce::canonicalize(root.as_ref())?;
        if !root.is_dir() {
            return Err(io::Error::new(io::ErrorKind::NotADirectory, "не папка"));
        }

        let mut files = Vec::new();
        let walker = WalkDir::new(&root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|e| e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.'));
        for entry in walker.filter_map(Result::ok) {
            if !entry.file_type().is_file() {
                continue;
            }
            let Ok(rel) = entry.path().strip_prefix(&root) else { continue };
            let parts: Option<Vec<&str>> = rel.components().map(|c| c.as_os_str().to_str()).collect();
            if let Some(parts) = parts {
                files.push(parts.join("/"));
            }
        }
        files.sort_by_key(|p| p.to_lowercase());
        Ok(Self::from_files(root, files))
    }

    /// Строит индекс из готового списка путей (удобно для тестов).
    pub fn from_files(root: PathBuf, files: Vec<String>) -> Self {
        let mut vault = Vault {
            root,
            notes: Vec::new(),
            note_by_path: HashMap::new(),
            note_by_stem: HashMap::new(),
            file_by_path: HashMap::new(),
            file_by_name: HashMap::new(),
        };
        for path in files {
            let lower = path.to_lowercase();
            if let Some(kind) = NoteKind::from_path(&path) {
                let idx = vault.notes.len();
                vault.note_by_path.insert(lower.clone(), idx);
                vault
                    .note_by_stem
                    .entry(paths::file_stem(&lower).to_owned())
                    .or_default()
                    .push(idx);
                vault.notes.push(Note {
                    title: paths::file_stem(&path).to_owned(),
                    path: path.clone(),
                    kind,
                });
            }
            vault
                .file_by_name
                .entry(paths::file_name(&lower).to_owned())
                .or_default()
                .push(path.clone());
            vault.file_by_path.insert(lower, path);
        }
        vault
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn name(&self) -> String {
        self.root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.root.display().to_string())
    }

    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    pub fn note(&self, path: &str) -> Option<&Note> {
        self.note_by_path.get(&path.to_lowercase()).map(|&i| &self.notes[i])
    }

    pub fn note_index(&self, note: &Note) -> Option<usize> {
        self.note_by_path.get(&note.path.to_lowercase()).copied()
    }

    /// Читает заметку (невалидный UTF-8 заменяется).
    pub fn read(&self, note: &Note) -> io::Result<String> {
        let bytes = fs::read(self.root.join(&note.path))?;
        Ok(String::from_utf8(bytes).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
    }

    /// Абсолютный путь к проиндексированному файлу.
    /// Файлы вне индекса (скрытые, вне корня) не отдаются никогда.
    pub fn file_path(&self, rel: &str) -> Option<PathBuf> {
        let rel = paths::join_normalize("", rel)?;
        let real = self.file_by_path.get(&rel.to_lowercase())?;
        Some(self.root.join(real))
    }

    /// Разрешает цель ссылки (`Заметка`, `папка/Заметка`, `../a.md`)
    /// относительно заметки `from`, как это делает Obsidian:
    /// сначала точный путь, затем поиск по имени файла.
    pub fn resolve_note(&self, target: &str, from: &str) -> Option<&Note> {
        let target = target.trim();
        if target.is_empty() {
            return None;
        }

        for base in [paths::parent(from), ""] {
            let Some(p) = paths::join_normalize(base, target) else { continue };
            let p = p.to_lowercase();
            if let Some(&i) = self.note_by_path.get(&p) {
                return Some(&self.notes[i]);
            }
            for ext in NOTE_EXTENSIONS {
                if let Some(&i) = self.note_by_path.get(&format!("{p}.{ext}")) {
                    return Some(&self.notes[i]);
                }
            }
        }

        // Поиск по имени: `[[Идея]]` найдёт `проекты/Идея.md`.
        let lower = target.to_lowercase().replace('\\', "/");
        let lower = lower.trim_start_matches("./").trim_start_matches('/');
        let has_note_ext = paths::extension(lower).is_some_and(|e| NOTE_EXTENSIONS.contains(&e.as_str()));
        let stem = if has_note_ext { paths::file_stem(lower) } else { paths::file_name(lower) };
        let without_ext = if has_note_ext { lower.rsplit_once('.').map_or(lower, |(s, _)| s) } else { lower };

        self.note_by_stem
            .get(stem)?
            .iter()
            .map(|&i| &self.notes[i])
            .filter(|n| {
                if !lower.contains('/') {
                    return true;
                }
                // `[[папка/Идея]]` — путь должен заканчиваться на указанный хвост.
                let p = n.path.to_lowercase();
                let p = p.rsplit_once('.').map_or(p.as_str(), |(s, _)| s);
                p == without_ext || p.ends_with(&format!("/{without_ext}"))
            })
            .min_by_key(|n| (n.path.matches('/').count(), n.path.len()))
    }

    /// Путь для новой заметки по вводу пользователя: `Идея`, `папка/Идея`,
    /// `страница.html`. Без расширения — Markdown. Запрещены выход за корень,
    /// скрытые сегменты и уже существующие заметки.
    pub fn new_note_path(&self, spec: &str) -> Result<String, String> {
        let path = checked_path(spec)?;
        let path = if NoteKind::from_path(&path).is_some() { path } else { format!("{path}.md") };
        if self.note(&path).is_some() || self.file_by_path.contains_key(&path.to_lowercase()) {
            return Err(format!("«{path}» уже существует"));
        }
        Ok(path)
    }

    /// Новый путь заметки `old` по вводу пользователя при переименовании или
    /// переносе: `Имя` (остаётся в своей папке), `папка/Имя`. Без расширения
    /// сохраняется прежнее. Регистр можно менять (`идея` → `Идея`).
    pub fn rename_path(&self, old: &str, spec: &str) -> Result<String, String> {
        let note = self.note(old).ok_or_else(|| format!("Нет заметки {old}"))?;
        let spec = spec.trim().replace('\\', "/");
        let spec = spec.trim_matches('/');
        let spec = if spec.contains('/') || spec.is_empty() { spec.to_owned() } else {
            let dir = paths::parent(&note.path);
            if dir.is_empty() { spec.to_owned() } else { format!("{dir}/{spec}") }
        };
        let path = checked_path(&spec)?;
        let path = if NoteKind::from_path(&path).is_some() {
            path
        } else {
            let ext = paths::extension(&note.path).unwrap_or_else(|| "md".into());
            format!("{path}.{ext}")
        };
        let lower = path.to_lowercase();
        if lower != note.path.to_lowercase() && self.file_by_path.contains_key(&lower) {
            return Err(format!("«{path}» уже существует"));
        }
        Ok(path)
    }

    /// Разрешает ссылку на любой файл (картинки и прочие вложения).
    pub fn resolve_file(&self, target: &str, from: &str) -> Option<&str> {
        let target = target.trim();
        if target.is_empty() {
            return None;
        }
        for base in [paths::parent(from), ""] {
            if let Some(p) = paths::join_normalize(base, target)
                && let Some(real) = self.file_by_path.get(&p.to_lowercase()) {
                    return Some(real);
                }
        }
        let name = paths::file_name(target).to_lowercase();
        self.file_by_name
            .get(&name)?
            .iter()
            .min_by_key(|p| (p.matches('/').count(), p.len()))
            .map(String::as_str)
    }
}

/// Проверяет путь новой/переименованной заметки из ввода пользователя (или из
/// сообщения заметки) и возвращает его в виде `папка/имя`. Пропускает только
/// обычные имена, которые одинаково безопасны в Linux и Windows: без `..`,
/// дисков (`C:`), потоков NTFS (`a.md:x`), служебных символов и имён
/// устройств (`CON`, `NUL`, `COM1`…), чтобы путь не мог уйти из хранилища.
fn checked_path(spec: &str) -> Result<String, String> {
    let spec = spec.trim().replace('\\', "/");
    let spec = spec.trim_matches('/');
    if spec.is_empty() {
        return Err("Пустое имя заметки".into());
    }
    for seg in spec.split('/') {
        if seg.is_empty() || seg.starts_with('.') || seg.trim() != seg || seg.ends_with('.') {
            return Err("Имя не может содержать пустые, скрытые (с точки), оканчивающиеся точкой или окружённые пробелами части пути".into());
        }
        if let Some(c) = seg.chars().find(|c| c.is_control() || r#"<>:"|?*"#.contains(*c)) {
            return Err(format!("Недопустимый символ в имени: «{}»", c.escape_default()));
        }
        let base = seg.split('.').next().unwrap_or(seg).trim_end().to_ascii_uppercase();
        let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
            || (base.len() == 4
                && (base.starts_with("COM") || base.starts_with("LPT"))
                && base.as_bytes()[3].is_ascii_digit());
        if reserved {
            return Err(format!("«{seg}» — зарезервированное имя устройства в Windows"));
        }
    }
    let path = paths::join_normalize("", spec).ok_or("Путь выходит за пределы хранилища")?;
    // Второй рубеж: только обычные компоненты, никаких корней и префиксов.
    if !Path::new(&path).components().all(|c| matches!(c, std::path::Component::Normal(_))) {
        return Err("Путь выходит за пределы хранилища".into());
    }
    Ok(path)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> Vault {
        Vault::from_files(
            PathBuf::from("/vault"),
            [
                "index.md",
                "Ideas/Rust.md",
                "Ideas/deep/Rust.md",
                "Web/Page.html",
                "img/cat.png",
                "Readme.markdown",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
        )
    }

    #[test]
    fn resolves_by_name_and_path() {
        let v = sample();
        assert_eq!(v.resolve_note("rust", "index.md").unwrap().path, "Ideas/Rust.md");
        assert_eq!(v.resolve_note("deep/Rust", "index.md").unwrap().path, "Ideas/deep/Rust.md");
        assert_eq!(v.resolve_note("Page", "index.md").unwrap().path, "Web/Page.html");
        assert_eq!(v.resolve_note("../index.md", "Web/Page.html").unwrap().path, "index.md");
        assert_eq!(v.resolve_note("Page.html", "x.md").unwrap().path, "Web/Page.html");
        assert_eq!(v.resolve_note("readme", "x.md").unwrap().path, "Readme.markdown");
        assert!(v.resolve_note("missing", "index.md").is_none());
    }

    #[test]
    fn new_note_paths() {
        let v = sample();
        assert_eq!(v.new_note_path(" Идея ").unwrap(), "Идея.md");
        assert_eq!(v.new_note_path("Web/Новая.html").unwrap(), "Web/Новая.html");
        assert_eq!(v.new_note_path("a/b/c").unwrap(), "a/b/c.md");
        assert!(v.new_note_path("index").is_err(), "уже есть index.md");
        assert!(v.new_note_path("Ideas/rust").is_err(), "регистр не важен");
        assert!(v.new_note_path("../x").is_err());
        assert!(v.new_note_path(".hidden").is_err());
        assert!(v.new_note_path("a//b").is_err());
        assert!(v.new_note_path("").is_err());
        // Пути, которые на Windows ушли бы из хранилища или в устройство.
        for bad in ["C:/Windows/x", "C:x", "a.md:поток", "CON", "nul.md", "папка/com1.txt", "lpt9", "a?b", "a.", "x\u{1}y"] {
            assert!(v.new_note_path(bad).is_err(), "{bad}");
            assert!(v.rename_path("index.md", bad).is_err(), "{bad}");
        }
        assert!(v.new_note_path("Console").is_ok() && v.new_note_path("com10").is_ok());
    }

    #[test]
    fn rename_paths() {
        let v = sample();
        assert_eq!(v.rename_path("Ideas/Rust.md", "Go").unwrap(), "Ideas/Go.md");
        assert_eq!(v.rename_path("Ideas/Rust.md", "Other/Go").unwrap(), "Other/Go.md");
        assert_eq!(v.rename_path("Web/Page.html", "Site").unwrap(), "Web/Site.html");
        assert_eq!(v.rename_path("Ideas/Rust.md", "rust").unwrap(), "Ideas/rust.md");
        assert_eq!(v.rename_path("index.md", "Index.md").unwrap(), "Index.md");
        assert!(v.rename_path("index.md", "Ideas/deep/Rust").is_err());
        assert!(v.rename_path("index.md", "../x").is_err());
        assert!(v.rename_path("index.md", " ").is_err());
        assert!(v.rename_path("nope.md", "x").is_err());
    }

    #[test]
    fn resolves_files() {
        let v = sample();
        assert_eq!(v.resolve_file("cat.png", "index.md"), Some("img/cat.png"));
        assert_eq!(v.resolve_file("../img/cat.png", "Web/Page.html"), Some("img/cat.png"));
        assert!(v.file_path("../etc/passwd").is_none());
        assert!(v.file_path("img/nope.png").is_none());
    }
}
