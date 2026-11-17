//! Состояние приложения: открытое хранилище и его граф.

use std::path::PathBuf;
use std::sync::{Mutex, RwLock};

use serde::Serialize;

use crate::core::graph::{self, Graph};
use crate::core::vault::Vault;

/// Открытое хранилище.
pub struct Session {
    pub vault: Vault,
    pub graph: Graph,
}

impl Session {
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, String> {
        let path = path.into();
        let vault = Vault::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let graph = graph::build(&vault);
        Ok(Self { vault, graph })
    }
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

impl Session {
    pub fn info(&self) -> VaultInfo<'_> {
        VaultInfo {
            name: self.vault.name(),
            root: self.vault.root().display().to_string(),
            base_url: super::protocol::BASE_URL,
            graph: &self.graph,
        }
    }
}
