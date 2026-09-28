use crate::{FILE_LIMIT, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Encoding {
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
    Ansi(u32),
}

impl std::fmt::Display for Encoding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Utf8 => write!(f, "UTF-8"),
            Self::Utf8Bom => write!(f, "UTF-8 BOM"),
            Self::Utf16Le => write!(f, "UTF-16 LE"),
            Self::Utf16Be => write!(f, "UTF-16 BE"),
            Self::Ansi(cp) => write!(f, "ANSI ({cp})"),
        }
    }
}

pub fn decode(bytes: &[u8], ansi: Option<u32>) -> Result<(String, Encoding)> {
    if bytes.len() > FILE_LIMIT {
        return Err("File exceeds the 20 MiB limit.".into());
    }
    let (text, encoding) = if let Some(cp) = ansi {
        (decode_ansi(bytes, cp)?, Encoding::Ansi(cp))
    } else if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        (utf8(rest)?, Encoding::Utf8Bom)
    } else if bytes.starts_with(&[0xff, 0xfe]) || bytes.starts_with(&[0xfe, 0xff]) {
        let le = bytes[0] == 0xff;
        let rest = &bytes[2..];
        if !rest.len().is_multiple_of(2) {
            return Err("Malformed UTF-16: odd byte count.".into());
        }
        let units: Vec<_> = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| {
                if le {
                    u16::from_le_bytes([p[0], p[1]])
                } else {
                    u16::from_be_bytes([p[0], p[1]])
                }
            })
            .collect();
        (
            String::from_utf16(&units).map_err(|_| "Malformed UTF-16 surrogate pair.")?,
            if le {
                Encoding::Utf16Le
            } else {
                Encoding::Utf16Be
            },
        )
    } else {
        (utf8(bytes)?, Encoding::Utf8)
    };
    validate_text(&text)?;
    Ok((text, encoding))
}

fn utf8(bytes: &[u8]) -> Result<String> {
    String::from_utf8(bytes.to_vec())
        .map_err(|_| "Not valid UTF-8. Use File > Open as ANSI for a legacy file.".into())
}

pub fn validate_text(text: &str) -> Result<()> {
    if text.contains('\0') {
        Err("Embedded NUL characters are not supported; the file was not changed.".into())
    } else {
        Ok(())
    }
}

pub fn encode(text: &str, encoding: Encoding) -> Result<Vec<u8>> {
    validate_text(text)?;
    let bytes = match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => [&[0xef, 0xbb, 0xbf][..], text.as_bytes()].concat(),
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let le = encoding == Encoding::Utf16Le;
            let mut bytes = if le {
                vec![0xff, 0xfe]
            } else {
                vec![0xfe, 0xff]
            };
            for unit in text.encode_utf16() {
                bytes.extend(if le {
                    unit.to_le_bytes()
                } else {
                    unit.to_be_bytes()
                });
            }
            bytes
        }
        Encoding::Ansi(cp) => encode_ansi(text, cp)?,
    };
    if bytes.len() > FILE_LIMIT {
        return Err(
            "Encoded file exceeds 20 MiB. Reduce the text or choose another encoding.".into(),
        );
    }
    Ok(bytes)
}

#[cfg(windows)]
fn decode_ansi(bytes: &[u8], cp: u32) -> Result<String> {
    use windows_sys::Win32::Globalization::*;
    if bytes.is_empty() {
        return Ok(String::new());
    }
    unsafe {
        let n = MultiByteToWideChar(
            cp,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        );
        if n == 0 {
            return Err(format!(
                "Cannot decode code page {cp}: {}",
                std::io::Error::last_os_error()
            ));
        }
        let mut buf = vec![0; n as usize];
        if MultiByteToWideChar(
            cp,
            MB_ERR_INVALID_CHARS,
            bytes.as_ptr(),
            bytes.len() as i32,
            buf.as_mut_ptr(),
            n,
        ) != n
        {
            return Err("ANSI conversion failed.".into());
        }
        String::from_utf16(&buf).map_err(|_| "Invalid Unicode from ANSI conversion.".into())
    }
}

#[cfg(windows)]
fn encode_ansi(text: &str, cp: u32) -> Result<Vec<u8>> {
    use windows_sys::Win32::Globalization::*;
    if text.is_empty() {
        return Ok(Vec::new());
    }
    if cp == 65001 {
        return Ok(text.as_bytes().to_vec());
    }
    let units: Vec<_> = text.encode_utf16().collect();
    unsafe {
        let mut used = 0;
        let n = WideCharToMultiByte(
            cp,
            WC_NO_BEST_FIT_CHARS,
            units.as_ptr(),
            units.len() as i32,
            std::ptr::null_mut(),
            0,
            std::ptr::null(),
            &mut used,
        );
        if n == 0 || used != 0 {
            return Err("Text cannot be represented in this ANSI code page. Choose UTF-8.".into());
        }
        let mut bytes = vec![0; n as usize];
        if WideCharToMultiByte(
            cp,
            WC_NO_BEST_FIT_CHARS,
            units.as_ptr(),
            units.len() as i32,
            bytes.as_mut_ptr(),
            n,
            std::ptr::null(),
            &mut used,
        ) != n
            || used != 0
        {
            return Err("ANSI conversion would lose characters. Choose UTF-8.".into());
        }
        if decode_ansi(&bytes, cp)? != text {
            return Err("ANSI conversion is not lossless. Choose UTF-8.".into());
        }
        Ok(bytes)
    }
}

#[cfg(not(windows))]
fn decode_ansi(_: &[u8], _: u32) -> Result<String> {
    Err("ANSI requires Windows.".into())
}
#[cfg(not(windows))]
fn encode_ansi(_: &str, _: u32) -> Result<Vec<u8>> {
    Err("ANSI requires Windows.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_roundtrips() {
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ] {
            for text in [
                "",
                "a\r\nb\nc\rd",
                "no final newline",
                "\u{1f600}e\u{301}\r\n",
            ] {
                let bytes = encode(text, encoding).unwrap();
                assert_eq!(decode(&bytes, None).unwrap(), (text.into(), encoding));
            }
        }
    }
    #[test]
    fn malformed_is_rejected() {
        for bytes in [&[0xff][..], &[0xff, 0xfe, 0], &[0xff, 0xfe, 0, 0xd8], &[0]] {
            assert!(decode(bytes, None).is_err());
        }
    }
    #[test]
    fn file_boundaries() {
        assert!(encode(&"a".repeat(FILE_LIMIT), Encoding::Utf8).is_ok());
        assert!(encode(&"a".repeat(FILE_LIMIT), Encoding::Utf8Bom).is_err());
        assert!(decode(&vec![b'a'; FILE_LIMIT + 1], None).is_err());
    }
    #[cfg(windows)]
    #[test]
    fn ansi_is_lossless() {
        assert_eq!(decode(&[0x80], Some(1252)).unwrap().0, "\u{20ac}");
        assert_eq!(
            encode("\u{20ac}", Encoding::Ansi(1252)).unwrap(),
            vec![0x80]
        );
        assert!(encode("\u{1f600}", Encoding::Ansi(1252)).is_err());
    }
}
