pub fn windows_version(version: &str) -> Result<[u16; 4], String> {
    let date = version
        .strip_prefix("0.1.")
        .ok_or("Version must use 0.1.YYYYMMDD.")?;
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Version must use 0.1.YYYYMMDD.".into());
    }
    let year: u16 = date[..4].parse().map_err(|_| "Invalid version year.")?;
    let month: u16 = date[4..6].parse().map_err(|_| "Invalid version month.")?;
    let day: u16 = date[6..].parse().map_err(|_| "Invalid version day.")?;
    let days = match month {
        2 if year.is_multiple_of(400) || (year.is_multiple_of(4) && !year.is_multiple_of(100)) => {
            29
        }
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        _ => 0,
    };
    if year < 2000 || day == 0 || day > days {
        return Err("Version date must be a real date in or after 2000.".into());
    }
    // PE and assembly version components are 16-bit; YYYYMMDD cannot fit in one component.
    Ok([0, 1, year, month * 100 + day])
}

pub fn manifest_architecture(architecture: &str) -> Result<&'static str, String> {
    match architecture {
        "x86" => Ok("x86"),
        "x86_64" => Ok("amd64"),
        "aarch64" => Ok("arm64"),
        _ => Err(format!("Unsupported Windows architecture: {architecture}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn date_versions_map_to_valid_pe_components() {
        assert_eq!(windows_version("0.1.20260928").unwrap(), [0, 1, 2026, 928]);
        assert_eq!(windows_version("0.1.20240229").unwrap(), [0, 1, 2024, 229]);
        for value in [
            "0.1.0",
            "0.2.20260928",
            "0.1.20260229",
            "0.1.20261301",
            "0.1.20260931",
            "0.1.20260001",
            "0.1.20260900",
            "0.1.20260928-dev",
            "0.1.２０２６",
        ] {
            assert!(windows_version(value).is_err(), "{value}");
        }
    }
    #[test]
    fn manifest_architectures_match_windows_names() {
        assert_eq!(manifest_architecture("x86").unwrap(), "x86");
        assert_eq!(manifest_architecture("x86_64").unwrap(), "amd64");
        assert_eq!(manifest_architecture("aarch64").unwrap(), "arm64");
        assert!(manifest_architecture("arm").is_err());
    }
}
