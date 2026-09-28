pub fn windows_version(version: &str) -> Result<[u16; 4], String> {
    let suffix = version
        .strip_prefix("0.1.")
        .ok_or("Version must use 0.1.YYYYMMDD or 0.1.YYYYMMDD.N.")?;
    let (date, revision) = match suffix.split_once('.') {
        Some((date, revision)) => {
            if revision.starts_with('0') || !revision.bytes().all(|b| b.is_ascii_digit()) {
                return Err("Release revision must be an integer from 1 to 65535.".into());
            }
            let revision = revision
                .parse::<u16>()
                .map_err(|_| "Release revision must be an integer from 1 to 65535.")?;
            (date, revision)
        }
        None => (suffix, 0),
    };
    if date.len() != 8 || !date.bytes().all(|b| b.is_ascii_digit()) {
        return Err("Version must use 0.1.YYYYMMDD or 0.1.YYYYMMDD.N.".into());
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
    // Four 16-bit fields retain date/revision ordering, including across year boundaries.
    // The display version remains 0.1.YYYYMMDD[.N].
    Ok([0, year, month * 100 + day, revision])
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
        assert_eq!(windows_version("0.1.20260928").unwrap(), [0, 2026, 928, 0]);
        assert_eq!(windows_version("0.1.20240229").unwrap(), [0, 2024, 229, 0]);
        assert_eq!(
            windows_version("0.1.20260928.1").unwrap(),
            [0, 2026, 928, 1]
        );
        assert_eq!(
            windows_version("0.1.20260928.65535").unwrap(),
            [0, 2026, 928, 65535]
        );
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
            "0.1.20260928.",
            "0.1.20260928.0",
            "0.1.20260928.01",
            "0.1.20260928.65536",
            "0.1.20260928.-1",
            "0.1.20260928.+1",
            "0.1.20260928.1.1",
            "0.1.20260928.１",
        ] {
            assert!(windows_version(value).is_err(), "{value}");
        }
    }
    #[test]
    fn numeric_versions_increase_across_revisions_and_dates() {
        let versions = [
            "0.1.20260928",
            "0.1.20260928.1",
            "0.1.20260928.9",
            "0.1.20260928.10",
            "0.1.20260928.65535",
            "0.1.20260929",
            "0.1.20261231.65535",
            "0.1.20270101",
        ];
        let mut previous = [0, 1, 2026, 928]; // Original published Windows version.
        for version in versions {
            let current = windows_version(version).unwrap();
            assert!(current > previous, "{version}");
            previous = current;
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
