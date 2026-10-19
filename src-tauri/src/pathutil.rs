//! Windows path comparison helpers shared by the storage and MCP code.
//!
//! They work on strings (not `std::path`) so the rules are identical, and
//! testable, on every platform.

/// Forward slashes, lower case, no `\\?\` prefix, no trailing separator.
pub fn norm_path(p: &str) -> String {
    let mut s = p.replace('\\', "/").to_lowercase();
    if let Some(rest) = s.strip_prefix("//?/") {
        s = rest.to_string();
    }
    s.trim_end_matches('/').to_string()
}

/// True when `path` is `root` or lies below it (component-wise, case-insensitive).
pub fn is_within(root: &str, path: &str) -> bool {
    let (r, p) = (norm_path(root), norm_path(path));
    !r.is_empty() && (p == r || p.starts_with(&format!("{r}/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_compared_component_wise_and_case_insensitively() {
        assert!(is_within("C:\\Users\\me", "c:/users/ME/docs/a.txt"));
        assert!(is_within("C:\\Users\\me\\", "\\\\?\\C:\\Users\\me\\docs"));
        assert!(!is_within("C:\\Users\\me", "C:\\Users\\me2\\a"));
        assert!(!is_within("", "C:\\a"));
        assert_eq!(norm_path("\\\\?\\C:\\A\\B\\"), "c:/a/b");
    }
}
