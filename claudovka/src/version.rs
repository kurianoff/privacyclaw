pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const GIT_HASH: &str = env!("GIT_HASH");
pub const BUILD_DATE: &str = env!("BUILD_DATE");

/// Full version string, e.g. `"0.2.0 (a1b2c3d 2026-03-08)"`.
#[allow(dead_code)]
pub fn version_string() -> String {
    format!("{} ({} {})", VERSION, GIT_HASH, BUILD_DATE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_string_format() {
        let s = version_string();
        // Must contain the semver
        assert!(s.contains(VERSION), "version string missing semver: {s}");
        // Must contain a parenthesised suffix
        assert!(s.contains('(') && s.contains(')'), "version string missing parens: {s}");
        // Build date must look like YYYY-MM-DD or be "unknown"
        let in_parens = s.split('(').nth(1).unwrap_or("").trim_end_matches(')');
        let parts: Vec<&str> = in_parens.split_whitespace().collect();
        assert_eq!(parts.len(), 2, "expected 2 parts inside parens: {s}");
        let date = parts[1];
        assert!(
            date == "unknown" || (date.len() == 10 && date.chars().nth(4) == Some('-')),
            "build date looks wrong: {date}"
        );
    }
}
