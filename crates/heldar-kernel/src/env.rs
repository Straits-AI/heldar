//! Small environment-variable helpers shared by the kernel + every app crate's config loader.
//! Single source of truth so parsing behavior (empty-string filtering, bool truthiness) stays
//! identical everywhere. Treats an empty/whitespace value the same as unset.

/// The trimmed value of `key`, or `None` when unset/empty.
pub fn var(key: &str) -> Option<String> {
    std::env::var(key).ok().filter(|s| !s.trim().is_empty())
}

/// `var(key)` or a string default.
pub fn var_or(key: &str, default: &str) -> String {
    var(key).unwrap_or_else(|| default.to_string())
}

/// Parse `key` into `T`, falling back to `default` when unset/empty/unparseable.
pub fn parse_or<T: std::str::FromStr>(key: &str, default: T) -> T {
    var(key).and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Truthy-string bool (`1|true|yes|on`), or `default` when unset.
pub fn parse_bool(key: &str, default: bool) -> bool {
    match var(key) {
        Some(v) => matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes" | "on"),
        None => default,
    }
}
