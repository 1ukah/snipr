//! Arquivos locais do usuário, na pasta dos ajustes:
//!
//! - `history/manual.jsonl` e `history/auto.jsonl`: uma linha JSON por busca
//!   concluída. Gravar é só acrescentar uma linha; o arquivo nunca é relido
//!   pelo app. Acima de `ROTATE_AT`, o arquivo vira `<nome>-<data>.jsonl`.
//! - `visited.json`: links de anúncios que o usuário abriu.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::Result;
use parking_lot::Mutex;
use serde::Serialize;

use crate::types::{Listing, SearchPlan};

const ROTATE_AT: u64 = 10 * 1024 * 1024;
/// Links abertos guardados (os mais antigos saem primeiro).
const MAX_VISITED: usize = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Manual,
    Auto,
}

impl Kind {
    fn file(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }
}

#[derive(Serialize)]
pub struct Entry<'a> {
    /// Fim da busca (segundos desde 1970).
    pub at: u64,
    pub query: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schedule: Option<&'a str>,
    pub sites: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<&'a SearchPlan>,
    pub elapsed_ms: u64,
    pub listings: &'a [Listing],
}

pub struct Store {
    dir: PathBuf,
    visited: Mutex<Vec<String>>,
    write: Mutex<()>,
}

/// Link sem consulta nem âncora: o mesmo anúncio pode chegar com parâmetros de rastreio.
pub fn link_key(url: &str) -> String {
    url.split(['?', '#']).next().unwrap_or(url).trim_end_matches('/').to_lowercase()
}

impl Store {
    pub fn open(dir: &Path) -> Self {
        let visited = std::fs::read(dir.join("visited.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        Self { dir: dir.to_path_buf(), visited: Mutex::new(visited), write: Mutex::new(()) }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn visited(&self) -> Vec<String> {
        self.visited.lock().clone()
    }

    pub fn visit(&self, url: &str) -> Result<()> {
        let key = link_key(url);
        let data = {
            let mut v = self.visited.lock();
            v.retain(|x| x != &key);
            v.push(key);
            let extra = v.len().saturating_sub(MAX_VISITED);
            v.drain(..extra);
            serde_json::to_vec(&*v)?
        };
        let _w = self.write.lock();
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join("visited.json.tmp");
        std::fs::write(&tmp, data)?;
        std::fs::rename(tmp, self.dir.join("visited.json"))?;
        Ok(())
    }

    pub fn append(&self, kind: Kind, entry: &Entry) -> Result<()> {
        let mut line = serde_json::to_vec(entry)?;
        line.push(b'\n');
        let _w = self.write.lock();
        let dir = self.dir.join("history");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{}.jsonl", kind.file()));
        if std::fs::metadata(&path).is_ok_and(|m| m.len() > ROTATE_AT) {
            let _ = std::fs::rename(&path, dir.join(format!("{}-{}.jsonl", kind.file(), entry.at)));
        }
        std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(&line)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grava_historico_e_links_abertos() {
        let dir = std::env::temp_dir().join(format!("snipr-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let s = Store::open(&dir);
        s.visit("https://sp.olx.com.br/a/b-123?lis=x").unwrap();
        s.visit("https://sp.olx.com.br/a/b-123/").unwrap();
        assert_eq!(s.visited(), vec!["https://sp.olx.com.br/a/b-123"]);
        assert_eq!(Store::open(&dir).visited().len(), 1, "fica no arquivo");
        let e = Entry { at: 1, query: "ps5", schedule: None, sites: &["olx".into()], plan: None, elapsed_ms: 10, listings: &[] };
        s.append(Kind::Manual, &e).unwrap();
        s.append(Kind::Manual, &e).unwrap();
        let text = std::fs::read_to_string(dir.join("history/manual.jsonl")).unwrap();
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().all(|l| serde_json::from_str::<serde_json::Value>(l).is_ok()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
