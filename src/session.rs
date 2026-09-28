use crate::{Result, TEXT_LIMIT, document::Document, file_io};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
};

const SESSION_LIMIT: usize = 700 * 1024 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub dark_mode: bool,
    pub restore: bool,
    pub wrap: bool,
    pub status: bool,
    pub face: String,
    pub font_points: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            dark_mode: false,
            restore: true,
            wrap: false,
            status: true,
            face: "Consolas".into(),
            font_points: 11,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub active: usize,
    pub settings: Settings,
    pub documents: Vec<Document>,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            version: 1,
            active: 0,
            settings: Settings::default(),
            documents: vec![Document::default()],
        }
    }
}

impl Session {
    pub fn snapshot(&self) -> Self {
        Self {
            version: self.version,
            active: self.active,
            settings: self.settings.clone(),
            documents: self.documents.iter().map(Document::snapshot).collect(),
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1 {
            return Err("Unsupported recovery version.".into());
        }
        if self.documents.len() > 256 {
            return Err("Recovery contains more than 256 tabs.".into());
        }
        if self.active >= self.documents.len().max(1) {
            return Err("Invalid active recovery tab.".into());
        }
        let mut total = 0usize;
        for d in &self.documents {
            d.text.validate()?;
            total = total
                .checked_add(d.text.bytes())
                .ok_or("Recovery text size overflow.")?;
            if total > TEXT_LIMIT {
                return Err("Recovery exceeds 100 MiB of decoded text.".into());
            }
            if !(10..=500).contains(&d.zoom) {
                return Err("Invalid recovery zoom.".into());
            }
        }
        if self.settings.face.encode_utf16().count() >= 32
            || !(6..=72).contains(&self.settings.font_points)
        {
            return Err("Invalid saved font settings.".into());
        }
        Ok(())
    }
}

pub fn directory() -> Result<PathBuf> {
    let root = std::env::var_os("LOCALAPPDATA")
        .ok_or("LOCALAPPDATA is not defined; session recovery is unavailable.")?;
    let path = PathBuf::from(root).join("RustNotepad");
    fs::create_dir_all(&path)
        .map_err(|e| format!("Create recovery directory {}: {e}", path.display()))?;
    Ok(path)
}

pub fn checkpoint(root: &Path, session: &Session) -> Result<()> {
    session.validate()?;
    let bytes = serde_json::to_vec(session).map_err(|e| format!("Serialize recovery: {e}"))?;
    if bytes.len() > SESSION_LIMIT {
        return Err("Recovery checkpoint exceeds its storage limit.".into());
    }
    let target = root.join("session.json");
    let old = if target.exists() {
        Some(file_io::read_bounded(
            fs::File::open(&target).map_err(|e| e.to_string())?,
            SESSION_LIMIT,
        )?)
    } else {
        None
    };
    if let Some(ref previous) = old {
        // Retain only a validated complete generation; never overwrite the fallback with corrupt data.
        if parse(previous).is_ok() {
            let fallback = root.join("session.previous.json");
            let expected = if fallback.exists() {
                Some(file_io::hash(
                    &fs::read(&fallback).map_err(|e| e.to_string())?,
                ))
            } else {
                None
            };
            file_io::write_transaction(&fallback, previous, expected)?;
        }
    }
    file_io::write_transaction(&target, &bytes, old.as_ref().map(|b| file_io::hash(b)))
}

fn parse(bytes: &[u8]) -> Result<Session> {
    let session: Session =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid recovery JSON: {e}"))?;
    session.validate()?;
    Ok(session)
}

pub fn load(root: &Path) -> Result<Option<Session>> {
    let path = root.join("session.json");
    if !path.exists() {
        return Ok(None);
    }
    let bytes = file_io::read_bounded(
        fs::File::open(&path).map_err(|e| e.to_string())?,
        SESSION_LIMIT,
    )?;
    parse(&bytes).map(Some)
}

pub fn load_previous(root: &Path) -> Result<Session> {
    let bytes = file_io::read_bounded(
        fs::File::open(root.join("session.previous.json")).map_err(|e| e.to_string())?,
        SESSION_LIMIT,
    )?;
    parse(&bytes)
}

pub fn clear_previous(root: &Path) -> Result<()> {
    let path = root.join("session.previous.json");
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("Cannot remove {}: {e}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn theme_setting_is_backward_compatible_and_persistent() {
        let legacy =
            r#"{"restore":true,"wrap":false,"status":true,"face":"Consolas","font_points":11}"#;
        let settings: Settings = serde_json::from_str(legacy).unwrap();
        assert!(!settings.dark_mode);
        let root = tempfile::tempdir().unwrap();
        let mut session = Session::default();
        session.settings.dark_mode = true;
        checkpoint(root.path(), &session).unwrap();
        let restored = load(root.path()).unwrap().unwrap();
        assert!(restored.settings.dark_mode);
        assert!(!restored.documents[0].dirty);
        session.settings.dark_mode = false;
        checkpoint(root.path(), &session).unwrap();
        assert!(!load(root.path()).unwrap().unwrap().settings.dark_mode);
    }
    #[test]
    fn restores_dirty_tabs_and_previous_generation() {
        let root = tempfile::tempdir().unwrap();
        let mut s = Session::default();
        s.documents[0].update("unsaved\ntext".into(), 0).unwrap();
        checkpoint(root.path(), &s).unwrap();
        assert!(load(root.path()).unwrap().unwrap().documents[0].dirty);
        s.documents[0].update("new".into(), 0).unwrap();
        checkpoint(root.path(), &s).unwrap();
        fs::write(root.path().join("session.json"), b"{").unwrap();
        assert!(load(root.path()).is_err());
        assert_eq!(
            load_previous(root.path()).unwrap().documents[0].text.body,
            "unsaved\ntext"
        );
    }
    #[test]
    fn rejects_invalid_metadata() {
        let mut s = Session::default();
        s.documents[0].text.endings.push(crate::document::Eol::Cr);
        assert!(s.validate().is_err());
        s = Session::default();
        s.version = 2;
        assert!(s.validate().is_err());
    }
}
