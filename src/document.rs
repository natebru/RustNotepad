use crate::{
    Result, TEXT_LIMIT,
    encoding::{self, Encoding},
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Eol {
    CrLf,
    Lf,
    Cr,
}

impl Eol {
    pub fn text(self) -> &'static str {
        match self {
            Self::CrLf => "\r\n",
            Self::Lf => "\n",
            Self::Cr => "\r",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Text {
    pub body: String,
    pub endings: Vec<Eol>,
    pub default_eol: Eol,
}

impl Text {
    pub fn parse(raw: &str) -> Self {
        let mut body = String::with_capacity(raw.len());
        let mut endings = Vec::new();
        let mut chars = raw.chars().peekable();
        while let Some(c) = chars.next() {
            if c == '\r' {
                let eol = if chars.peek() == Some(&'\n') {
                    chars.next();
                    Eol::CrLf
                } else {
                    Eol::Cr
                };
                endings.push(eol);
                body.push('\n');
            } else {
                if c == '\n' {
                    endings.push(Eol::Lf);
                }
                body.push(c);
            }
        }
        let mut default_eol = Eol::CrLf;
        let mut count = 0;
        for eol in [Eol::CrLf, Eol::Lf, Eol::Cr] {
            let n = endings.iter().filter(|&&e| e == eol).count();
            if n > count {
                count = n;
                default_eol = eol;
            }
        }
        Self {
            body,
            endings,
            default_eol,
        }
    }

    pub fn raw(&self) -> String {
        let mut raw = String::with_capacity(self.body.len() + self.endings.len());
        let mut endings = self.endings.iter();
        for c in self.body.chars() {
            if c == '\n' {
                raw.push_str(endings.next().unwrap_or(&self.default_eol).text());
            } else {
                raw.push(c);
            }
        }
        raw
    }

    pub fn bytes(&self) -> usize {
        crate::utf16_len(&self.body) * 2
    }

    pub fn validate(&self) -> Result<()> {
        encoding::validate_text(&self.body)?;
        if self.body.contains('\r')
            || self.body.bytes().filter(|&c| c == b'\n').count() != self.endings.len()
        {
            return Err("Recovery data has inconsistent line endings.".into());
        }
        if self.bytes() > TEXT_LIMIT {
            return Err("Recovery text exceeds the text limit.".into());
        }
        Ok(())
    }

    pub fn eol_label(&self) -> &'static str {
        if self.endings.iter().any(|&e| e != self.default_eol) {
            "Mixed EOL"
        } else {
            match self.default_eol {
                Eol::CrLf => "CRLF",
                Eol::Lf => "LF",
                Eol::Cr => "CR",
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Edit {
    start: usize,
    removed: String,
    inserted: String,
    ending_start: usize,
    removed_endings: Vec<Eol>,
    inserted_endings: Vec<Eol>,
    before_revision: u64,
    after_revision: u64,
    before_eol: Eol,
    after_eol: Eol,
}

impl Edit {
    fn bytes(&self) -> usize {
        self.removed.len()
            + self.inserted.len()
            + self.removed_endings.len()
            + self.inserted_endings.len()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Document {
    pub path: Option<PathBuf>,
    pub text: Text,
    pub encoding: Encoding,
    pub dirty: bool,
    pub disk_hash: Option<[u8; 32]>,
    pub selection: (i32, i32),
    pub scroll: (i32, i32),
    pub zoom: i32,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    next_revision: u64,
    #[serde(default = "initial_savepoint")]
    saved_revision: Option<u64>,
    #[serde(skip)]
    undo: Vec<Edit>,
    #[serde(skip)]
    redo: Vec<Edit>,
}

fn initial_savepoint() -> Option<u64> {
    Some(0)
}

impl Default for Document {
    fn default() -> Self {
        Self {
            path: None,
            text: Text::parse(""),
            encoding: Encoding::Utf8,
            dirty: false,
            disk_hash: None,
            selection: (0, 0),
            scroll: (0, 0),
            zoom: 100,
            revision: 0,
            next_revision: 0,
            saved_revision: Some(0),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }
}

impl Document {
    pub fn snapshot(&self) -> Self {
        Self {
            path: self.path.clone(),
            text: self.text.clone(),
            encoding: self.encoding,
            dirty: self.dirty,
            disk_hash: self.disk_hash,
            selection: self.selection,
            scroll: self.scroll,
            zoom: self.zoom,
            revision: self.revision,
            next_revision: self.next_revision,
            saved_revision: self.saved_revision,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn from_text(raw: &str, encoding: Encoding) -> Self {
        Self {
            text: Text::parse(raw),
            encoding,
            ..Self::default()
        }
    }

    pub fn title(&self) -> String {
        let name = self
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or("Untitled".into());
        format!("{}{}", if self.dirty { "*" } else { "" }, name)
    }

    pub fn update(&mut self, body: String, other_bytes: usize) -> Result<bool> {
        self.update_hint(body, other_bytes, None)
    }

    pub fn update_hint(
        &mut self,
        body: String,
        other_bytes: usize,
        hint: Option<(usize, usize, bool)>,
    ) -> Result<bool> {
        encoding::validate_text(&body)?;
        if body.contains('\r') {
            return Err("Editor returned noncanonical line endings.".into());
        }
        if crate::utf16_len(&body) * 2 + other_bytes > TEXT_LIMIT {
            return Err("The 100 MiB decoded-text limit would be exceeded.".into());
        }
        if body == self.text.body {
            return Ok(false);
        }
        let old = &self.text.body;
        let hinted = hint.and_then(|(start, end, backward)| {
            let mut start = crate::search::utf16_to_byte(old, start);
            let mut end = crate::search::utf16_to_byte(old, end);
            if start == end && body.len() < old.len() {
                let deleted = old.len() - body.len();
                if backward {
                    start = start.saturating_sub(deleted);
                } else {
                    end = end.saturating_add(deleted).min(old.len());
                }
            }
            if start <= end
                && old.is_char_boundary(start)
                && old.is_char_boundary(end)
                && body.len() >= start + old.len() - end
                && body.starts_with(&old[..start])
                && body.ends_with(&old[end..])
            {
                Some((start, old.len() - end))
            } else {
                None
            }
        });
        let (prefix, suffix) = hinted.unwrap_or_else(|| {
            let mut prefix = old
                .bytes()
                .zip(body.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            while !old.is_char_boundary(prefix) || !body.is_char_boundary(prefix) {
                prefix -= 1;
            }
            let mut suffix = old[prefix..]
                .bytes()
                .rev()
                .zip(body[prefix..].bytes().rev())
                .take_while(|(a, b)| a == b)
                .count();
            while !old.is_char_boundary(old.len() - suffix)
                || !body.is_char_boundary(body.len() - suffix)
            {
                suffix -= 1;
            }
            (prefix, suffix)
        });
        let start_eol = old[..prefix].bytes().filter(|&b| b == b'\n').count();
        let removed = &old[prefix..old.len() - suffix];
        let inserted = &body[prefix..body.len() - suffix];
        let old_count = removed.bytes().filter(|&b| b == b'\n').count();
        let new_count = inserted.bytes().filter(|&b| b == b'\n').count();
        let preserved = if old_count == new_count {
            self.text.endings[start_eol..start_eol + old_count].to_vec()
        } else {
            vec![self.text.default_eol; new_count]
        };
        let edit = Edit {
            start: prefix,
            removed: removed.into(),
            inserted: inserted.into(),
            ending_start: start_eol,
            removed_endings: self.text.endings[start_eol..start_eol + old_count].to_vec(),
            inserted_endings: preserved,
            before_revision: self.revision,
            after_revision: self.next_revision.wrapping_add(1),
            before_eol: self.text.default_eol,
            after_eol: self.text.default_eol,
        };
        self.text.endings.splice(
            start_eol..start_eol + old_count,
            edit.inserted_endings.iter().copied(),
        );
        self.text.body = body;
        self.revision = edit.after_revision;
        self.next_revision = self.revision;
        self.dirty = self.saved_revision != Some(self.revision);
        self.redo.clear();
        self.undo.push(edit);
        self.trim_undo();
        Ok(true)
    }

    pub fn undo(&mut self, redo: bool, other_bytes: usize) -> Result<()> {
        let source = if redo { &mut self.redo } else { &mut self.undo };
        let Some(edit) = source.last() else {
            return Ok(());
        };
        let (remove, insert) = if redo {
            (&edit.removed, &edit.inserted)
        } else {
            (&edit.inserted, &edit.removed)
        };
        let bytes = self.text.bytes() - remove.encode_utf16().count() * 2
            + insert.encode_utf16().count() * 2;
        if bytes + other_bytes > TEXT_LIMIT {
            return Err("Undo/redo would exceed the text limit.".into());
        }
        let edit = source.pop().unwrap();
        let (remove, insert, remove_eols, insert_eols) = if redo {
            (
                &edit.removed,
                &edit.inserted,
                &edit.removed_endings,
                &edit.inserted_endings,
            )
        } else {
            (
                &edit.inserted,
                &edit.removed,
                &edit.inserted_endings,
                &edit.removed_endings,
            )
        };
        self.text
            .body
            .replace_range(edit.start..edit.start + remove.len(), insert);
        self.text.endings.splice(
            edit.ending_start..edit.ending_start + remove_eols.len(),
            insert_eols.iter().copied(),
        );
        let cursor = self.text.body[..edit.start + insert.len()]
            .encode_utf16()
            .count() as i32;
        self.selection = (cursor, cursor);
        self.revision = if redo {
            edit.after_revision
        } else {
            edit.before_revision
        };
        self.text.default_eol = if redo {
            edit.after_eol
        } else {
            edit.before_eol
        };
        self.dirty = self.saved_revision != Some(self.revision);
        if redo {
            self.undo.push(edit);
        } else {
            self.redo.push(edit);
        }
        Ok(())
    }

    pub fn mark_saved(&mut self) {
        self.saved_revision = Some(self.revision);
        self.dirty = false;
    }

    pub fn invalidate_savepoint(&mut self) {
        self.saved_revision = None;
        self.dirty = true;
    }

    pub fn convert_eol(&mut self, eol: Eol) {
        if self.text.default_eol == eol && self.text.endings.iter().all(|&e| e == eol) {
            return;
        }
        let edit = Edit {
            start: 0,
            removed: String::new(),
            inserted: String::new(),
            ending_start: 0,
            removed_endings: self.text.endings.clone(),
            inserted_endings: vec![eol; self.text.endings.len()],
            before_revision: self.revision,
            after_revision: self.next_revision.wrapping_add(1),
            before_eol: self.text.default_eol,
            after_eol: eol,
        };
        self.text.endings.clone_from(&edit.inserted_endings);
        self.text.default_eol = eol;
        self.revision = edit.after_revision;
        self.next_revision = self.revision;
        self.dirty = self.saved_revision != Some(self.revision);
        self.redo.clear();
        self.undo.push(edit);
        self.trim_undo();
    }

    fn trim_undo(&mut self) {
        let mut size: usize = self.undo.iter().map(Edit::bytes).sum();
        while size > 32 * 1024 * 1024 && self.undo.len() > 1 {
            size -= self.undo.remove(0).bytes();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn boundaries_survive_edit_and_undo() {
        let raw = "a\r\nb\nc\rd\u{1f600}";
        let mut doc = Document::from_text(raw, Encoding::Utf8);
        assert_eq!(doc.text.raw(), raw);
        doc.update("a\nb\nc\nd\u{1f600}!".into(), 0).unwrap();
        assert_eq!(doc.text.raw(), format!("{raw}!"));
        doc.update("a\nB\nC\nd\u{1f600}!".into(), 0).unwrap();
        doc.undo(false, 0).unwrap();
        doc.undo(false, 0).unwrap();
        assert_eq!(doc.text.raw(), raw);
        doc.undo(true, 0).unwrap();
        assert_eq!(doc.text.raw(), format!("{raw}!"));
    }
    #[test]
    fn refusal_does_not_mutate() {
        let mut d = Document::default();
        assert!(d.update("x".into(), TEXT_LIMIT).is_err());
        assert_eq!(d.text.raw(), "");
        assert!(!d.dirty);
    }
    #[test]
    fn unicode_diff_and_newline_replacement() {
        let mut d = Document::from_text("\u{1f600}\r\nlast", Encoding::Utf8);
        d.update("\u{1f601}\nlast\n".into(), 0).unwrap();
        d.undo(false, 0).unwrap();
        assert_eq!(d.text.raw(), "\u{1f600}\r\nlast");
    }

    #[test]
    fn repeated_lines_preserve_the_actual_deleted_boundary() {
        let mut d = Document::from_text("a\r\nsame\nsame\rend", Encoding::Utf8);
        d.update_hint("a\nsame\nend".into(), 0, Some((2, 7, false)))
            .unwrap();
        assert_eq!(d.text.raw(), "a\r\nsame\rend");
        d.undo(false, 0).unwrap();
        assert_eq!(d.text.raw(), "a\r\nsame\nsame\rend");
        d.update_hint("a\nsame\nsameend".into(), 0, Some((12, 12, true)))
            .unwrap();
        assert_eq!(d.text.raw(), "a\r\nsame\nsameend");
    }

    #[test]
    fn savepoints_and_eol_conversion_are_undoable() {
        let mut d = Document::from_text("a\r\nb\n", Encoding::Utf8);
        d.update("a\nb\nc".into(), 0).unwrap();
        assert!(d.dirty);
        d.mark_saved();
        d.update("a\nb\ncd".into(), 0).unwrap();
        d.undo(false, 0).unwrap();
        assert!(!d.dirty);
        d.convert_eol(Eol::Lf);
        assert!(d.dirty);
        assert_eq!(d.text.raw(), "a\nb\nc");
        d.undo(false, 0).unwrap();
        assert_eq!(d.text.raw(), "a\r\nb\nc");
        assert!(!d.dirty);
        d.undo(true, 0).unwrap();
        assert!(d.dirty);
    }

    #[test]
    fn aggregate_limit_is_exact_and_unicode_safe() {
        let mut d = Document::default();
        assert!(d.update("\u{1f600}".into(), TEXT_LIMIT - 3).is_err());
        assert!(d.update("\u{1f600}".into(), TEXT_LIMIT - 4).unwrap());
        let snapshot = d.text.clone();
        assert!(d.update("\u{1f600}x".into(), TEXT_LIMIT - 4).is_err());
        assert_eq!(d.text, snapshot);
        d.undo(false, TEXT_LIMIT - 4).unwrap();
        assert_eq!(d.text.body, "");
        assert!(d.undo(true, TEXT_LIMIT).is_err());
        assert_eq!(d.text.body, "");
    }
}
