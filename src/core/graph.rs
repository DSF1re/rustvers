//! Граф связей между заметками.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::links;
use super::paths;
use super::vault::{Note, NoteKind, Vault};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Markdown,
    Html,
    /// Ссылка есть, а заметки нет (как «призрачные» узлы в Obsidian).
    Missing,
}

impl From<NoteKind> for NodeKind {
    fn from(k: NoteKind) -> Self {
        match k {
            NoteKind::Markdown => Self::Markdown,
            NoteKind::Html => Self::Html,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Node {
    /// Путь заметки; для отсутствующих — `?цель`.
    pub id: String,
    pub title: String,
    pub kind: NodeKind,
}

/// Граф: рёбра — пары индексов `[откуда, куда]`, без дублей и петель.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<[usize; 2]>,
}

/// Расширения, которые считаются заметками при создании «призраков».
fn looks_like_note(target: &str) -> bool {
    match paths::extension(target) {
        None => true,
        Some(ext) => matches!(ext.as_str(), "md" | "markdown" | "html" | "htm") || ext.contains(' '),
    }
}

/// Строит граф, читая все заметки с диска.
pub fn build(vault: &Vault) -> Graph {
    build_from(vault, |note| vault.read(note).map(|src| links::extract(note.kind, &src)).unwrap_or_default())
}

/// Строит граф по уже известным ссылкам каждой заметки (без чтения файлов).
pub fn build_from(vault: &Vault, mut targets: impl FnMut(&Note) -> Vec<String>) -> Graph {
    let mut nodes: Vec<Node> = vault
        .notes()
        .iter()
        .map(|n| Node { id: n.path.clone(), title: n.title.clone(), kind: n.kind.into() })
        .collect();
    let mut missing: HashMap<String, usize> = HashMap::new();
    let mut edges = HashSet::new();

    for (from, note) in vault.notes().iter().enumerate() {
        for target in targets(note) {
            let to = if let Some(n) = vault.resolve_note(&target, &note.path) {
                vault.note_index(n)
            } else if vault.resolve_file(&target, &note.path).is_some() || !looks_like_note(&target) {
                None // вложение (картинка, pdf…) — не узел графа
            } else {
                let title = paths::file_stem(target.trim()).to_owned();
                let key = title.to_lowercase();
                Some(*missing.entry(key.clone()).or_insert_with(|| {
                    nodes.push(Node { id: format!("?{key}"), title, kind: NodeKind::Missing });
                    nodes.len() - 1
                }))
            };
            if let Some(to) = to.filter(|&to| to != from) {
                edges.insert([from, to]);
            }
        }
    }

    let mut edges: Vec<_> = edges.into_iter().collect();
    edges.sort_unstable();
    Graph { nodes, edges }
}
