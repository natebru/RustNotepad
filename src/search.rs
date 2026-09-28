use crate::Result;

pub fn matches(text: &str, needle: &str, case: bool) -> Result<Vec<std::ops::Range<usize>>> {
    Ok(ranges(text, needle, case)?.collect())
}

pub fn ranges<'a>(
    text: &'a str,
    needle: &'a str,
    case: bool,
) -> Result<Box<dyn Iterator<Item = std::ops::Range<usize>> + 'a>> {
    if needle.is_empty() {
        return Err("Enter text to find.".into());
    }
    if case {
        Ok(Box::new(
            text.match_indices(needle).map(|(i, _)| i..i + needle.len()),
        ))
    } else {
        let wanted: Vec<_> = needle.chars().flat_map(char::to_lowercase).collect();
        let mut after = 0;
        Ok(Box::new(text.char_indices().filter_map(
            move |(start, _)| {
                if start < after {
                    return None;
                }
                let mut wanted = wanted.iter();
                for (offset, c) in text[start..].char_indices() {
                    for lower in c.to_lowercase() {
                        if wanted.next() != Some(&lower) {
                            return None;
                        }
                    }
                    if wanted.len() == 0 {
                        after = start + offset + c.len_utf8();
                        return Some(start..after);
                    }
                }
                None
            },
        )))
    }
}

pub fn find(
    text: &str,
    needle: &str,
    from: usize,
    case: bool,
    backward: bool,
    wrap: bool,
) -> Result<Option<std::ops::Range<usize>>> {
    let mut found = ranges(text, needle, case)?;
    let result = if backward {
        found.by_ref().take_while(|r| r.end <= from).last()
    } else {
        found.find(|r| r.start >= from)
    };
    if result.is_some() || !wrap {
        return Ok(result);
    }
    let mut again = ranges(text, needle, case)?;
    Ok(if backward { again.last() } else { again.next() })
}

pub fn utf16_to_byte(text: &str, offset: usize) -> usize {
    if text.is_ascii() {
        return offset.min(text.len());
    }
    let mut n = 0;
    for (i, c) in text.char_indices() {
        if n + c.len_utf16() > offset {
            return i;
        }
        n += c.len_utf16();
    }
    text.len()
}

pub fn line_column(text: &str, utf16: usize) -> (usize, usize) {
    let prefix = &text[..utf16_to_byte(text, utf16)];
    (
        prefix.bytes().filter(|&b| b == b'\n').count() + 1,
        prefix.rsplit('\n').next().unwrap_or("").chars().count() + 1,
    )
}

pub fn line_offset(text: &str, line: usize) -> Result<usize> {
    if line == 0 {
        return Err("Line numbers start at 1.".into());
    }
    if line == 1 {
        return Ok(0);
    }
    text.match_indices('\n')
        .nth(line - 2)
        .map(|(i, _)| i + 1)
        .ok_or_else(|| "That line does not exist.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unicode_selection_and_find() {
        let s = "a\u{1f600}\nHello HELLO";
        assert_eq!(utf16_to_byte(s, 2), 1);
        assert_eq!(line_column(s, 3), (1, 3));
        assert_eq!(matches(s, "hello", false).unwrap().len(), 2);
        assert_eq!(
            find(s, "Hello", s.len(), true, false, true).unwrap(),
            Some(6..11)
        );
        assert!(matches(s, "", true).is_err());
        assert_eq!(line_offset(s, 2).unwrap(), 6);
        assert!(line_offset(s, 3).is_err());
    }
}
