use crate::{FILE_LIMIT, Result};
use sha2::{Digest, Sha256};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static SEQUENCE: AtomicU64 = AtomicU64::new(0);

pub fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn read(path: &Path) -> Result<Vec<u8>> {
    let file = File::open(path).map_err(|e| format!("Open {}: {e}", path.display()))?;
    read_bounded(file, FILE_LIMIT).map_err(|e| format!("Read {}: {e}", path.display()))
}

pub fn read_bounded(reader: impl Read, limit: usize) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err(format!("Input exceeds the {} byte limit.", limit));
    }
    Ok(bytes)
}

pub fn temporary(path: &Path, suffix: &str) -> PathBuf {
    let n = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    path.with_file_name(format!(
        ".{name}.rustnotepad-{}-{n}.{suffix}",
        std::process::id()
    ))
}

pub fn save(path: &Path, bytes: &[u8], expected: Option<[u8; 32]>) -> Result<()> {
    if bytes.len() > FILE_LIMIT {
        return Err("Encoded file exceeds 20 MiB.".into());
    }
    write_transaction(path, bytes, expected)
}

pub fn write_transaction(path: &Path, bytes: &[u8], expected: Option<[u8; 32]>) -> Result<()> {
    let temp = temporary(path, "tmp");
    let backup = temporary(path, "bak");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(|e| format!("Create {}: {e}", temp.display()))?;
    output
        .write_all(bytes)
        .and_then(|_| output.sync_all())
        .map_err(|e| {
            format!(
                "Write/flush failed: {e}. Destination untouched; temporary file: {}",
                temp.display()
            )
        })?;
    drop(output);
    let operation = (|| {
        let guard = if let Some(expected) = expected {
            let mut options = OpenOptions::new();
            options.read(true);
            #[cfg(windows)]
            {
                use std::os::windows::fs::OpenOptionsExt;
                use windows_sys::Win32::Storage::FileSystem::{FILE_SHARE_DELETE, FILE_SHARE_READ};
                options.share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE);
            }
            let file = options
                .open(path)
                .map_err(|e| format!("Cannot lock original {}: {e}", path.display()))?;
            let current = read_bounded(&file, 700 * 1024 * 1024)?;
            if hash(&current) != expected {
                return Err(
                    "The file changed on disk. Reload or use Save As; no overwrite was performed."
                        .into(),
                );
            }
            Some(file)
        } else {
            None
        };
        promote(path, &temp, &backup, expected.is_some())?;
        drop(guard);
        if backup.exists() {
            if let Some(expected) = expected {
                let prior = read_bounded(
                    File::open(&backup)
                        .map_err(|e| format!("Cannot verify recovery backup: {e}"))?,
                    700 * 1024 * 1024,
                )?;
                if hash(&prior) != expected {
                    return Err("The destination was replaced concurrently during save. The competing version is preserved in the backup; review both files before saving again.".into());
                }
            }
            std::fs::remove_file(&backup).map_err(|e| {
                format!(
                    "Saved, but cannot remove recovery backup {}: {e}",
                    backup.display()
                )
            })?;
        }
        Ok(())
    })();
    operation.map_err(|e: String| {
        format!(
            "{e}\nRecoverable paths (if present):\n{}\n{}\n{}",
            path.display(),
            temp.display(),
            backup.display()
        )
    })
}

#[cfg(windows)]
fn promote(path: &Path, temp: &Path, backup: &Path, existing: bool) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::*;
    let wide = |p: &Path| {
        p.as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>()
    };
    let (path, temp, backup) = (wide(path), wide(temp), wide(backup));
    unsafe {
        let ok = if existing {
            ReplaceFileW(
                path.as_ptr(),
                temp.as_ptr(),
                backup.as_ptr(),
                0,
                std::ptr::null(),
                std::ptr::null(),
            )
        } else {
            MoveFileExW(temp.as_ptr(), path.as_ptr(), MOVEFILE_WRITE_THROUGH)
        };
        if ok == 0 {
            return Err(format!(
                "File replacement failed: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn promote(path: &Path, temp: &Path, backup: &Path, existing: bool) -> Result<()> {
    if existing {
        std::fs::copy(path, backup).map_err(|e| e.to_string())?;
    } else if path.exists() {
        return Err("Destination already exists.".into());
    }
    std::fs::rename(temp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn create_replace_and_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("test.txt");
        save(&p, b"old", None).unwrap();
        assert!(save(&p, b"bad", None).is_err());
        assert!(save(&p, b"bad", Some(hash(b"other"))).is_err());
        assert_eq!(read(&p).unwrap(), b"old");
        save(&p, b"new", Some(hash(b"old"))).unwrap();
        assert_eq!(read(&p).unwrap(), b"new");
    }
    #[test]
    fn bounded_stream() {
        assert!(read_bounded(&b"1234"[..], 3).is_err());
        assert_eq!(read_bounded(&b"123"[..], 3).unwrap(), b"123");
    }
    #[cfg(windows)]
    #[test]
    fn denied_writes_leave_original() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("readonly.txt");
        save(&p, b"old", None).unwrap();
        let original_permissions = std::fs::metadata(&p).unwrap().permissions();
        let mut permissions = original_permissions.clone();
        permissions.set_readonly(true);
        std::fs::set_permissions(&p, permissions.clone()).unwrap();
        assert!(save(&p, b"new", Some(hash(b"old"))).is_err());
        assert_eq!(read(&p).unwrap(), b"old");
        std::fs::set_permissions(&p, original_permissions).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn concurrent_writer_and_missing_parent_are_explicit_failures() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("locked.txt");
        save(&p, b"old", None).unwrap();
        let writer = OpenOptions::new()
            .write(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&p)
            .unwrap();
        assert!(save(&p, b"new", Some(hash(b"old"))).is_err());
        assert_eq!(read(&p).unwrap(), b"old");
        drop(writer);
        save(&p, b"new", Some(hash(b"old"))).unwrap();
        assert!(save(&dir.path().join("missing").join("file.txt"), b"text", None).is_err());
    }
}
