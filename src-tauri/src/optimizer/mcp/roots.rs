//! Locations the user has authorised for Windows MCP search.
//!
//! The list starts empty. Nothing is searched, and no connector call is made,
//! outside these folders — whatever Windows itself would allow — so "search my
//! PC" can never silently turn into "read unrelated user directories".

use crate::db::Database;

use super::policy::{is_within, norm_path, validate_path_arg};

const KEY: &str = "mcp.searchRoots";
pub const MAX_ROOTS: usize = 16;

/// What validation needs to know about the file system (injectable for tests).
pub trait RootFs {
    /// `Some(true)` for a plain directory, `Some(false)` for a reparse point /
    /// symlink, `None` when it does not exist or is not a directory.
    fn plain_directory(&self, path: &str) -> Option<bool>;
}

pub struct RealFs;
impl RootFs for RealFs {
    fn plain_directory(&self, path: &str) -> Option<bool> {
        let m = std::fs::symlink_metadata(path).ok()?;
        if m.file_type().is_symlink() {
            return Some(false);
        }
        if !m.is_dir() {
            return None;
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::MetadataExt;
            const REPARSE: u32 = 0x400;
            if m.file_attributes() & REPARSE != 0 {
                return Some(false);
            }
        }
        Some(true)
    }
}

/// Validates and normalises a folder the user wants to authorise.
pub fn validate_root(path: &str, fs: &dyn RootFs) -> Result<String, String> {
    let trimmed = path.trim().trim_matches('"').trim_end_matches(['\\', '/']);
    validate_path_arg(&format!("{trimmed}\\x")).map_err(|_| "enter a full folder path such as C:\\Users\\you\\Downloads".to_string())?;
    let normalised = trimmed.replace('/', "\\");
    if normalised.len() <= 2 {
        return Err("a whole drive cannot be authorised; pick a folder".into());
    }
    if crate::cleanup::is_protected_path(&format!("{normalised}\\x")) {
        return Err("system, program and application-data folders cannot be authorised".into());
    }
    match fs.plain_directory(&normalised) {
        Some(true) => Ok(normalised),
        Some(false) => Err("that folder is a junction or symbolic link; authorise the real folder instead".into()),
        None => Err("that folder does not exist".into()),
    }
}

pub fn list(db: &Database) -> Vec<String> {
    db.get_optimizer_setting(KEY).ok().flatten().and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok()).unwrap_or_default()
}

fn save(db: &Database, roots: &[String]) -> Result<(), String> {
    db.set_optimizer_setting(KEY, &serde_json::to_string(roots).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn add(db: &Database, path: &str, fs: &dyn RootFs) -> Result<Vec<String>, String> {
    let root = validate_root(path, fs)?;
    let mut roots = list(db);
    if roots.iter().any(|r| norm_path(r) == norm_path(&root)) {
        return Ok(roots);
    }
    if roots.iter().any(|r| is_within(r, &root)) {
        return Err("that folder is already covered by an authorised folder".into());
    }
    if roots.len() >= MAX_ROOTS {
        return Err(format!("at most {MAX_ROOTS} folders can be authorised"));
    }
    // A new parent absorbs the narrower folders it contains.
    roots.retain(|r| !is_within(&root, r));
    roots.push(root);
    save(db, &roots)?;
    Ok(roots)
}

pub fn remove(db: &Database, path: &str) -> Result<Vec<String>, String> {
    let mut roots = list(db);
    let before = roots.len();
    roots.retain(|r| norm_path(r) != norm_path(path.trim()));
    if roots.len() == before {
        return Err("that folder is not in the list".into());
    }
    save(db, &roots)?;
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fs(&'static [(&'static str, bool)]);
    impl RootFs for Fs {
        fn plain_directory(&self, path: &str) -> Option<bool> {
            self.0.iter().find(|(p, _)| p.eq_ignore_ascii_case(path)).map(|(_, plain)| *plain)
        }
    }
    const FS: Fs = Fs(&[("C:\\Users\\me\\Downloads", true), ("C:\\Users\\me\\Downloads\\Old", true), ("C:\\Users\\me\\Link", false), ("C:\\Windows\\System32", true), ("C:\\Users\\me\\AppData\\Local\\Temp", true), ("C:\\Users\\me\\Documents", true)]);

    fn db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite3")).unwrap();
        db.migrate().unwrap();
        (dir, db)
    }

    #[test]
    fn only_plain_user_folders_can_be_authorised() {
        assert_eq!(validate_root("\"C:\\Users\\me\\Downloads\\\"", &FS).unwrap(), "C:\\Users\\me\\Downloads");
        assert_eq!(validate_root("C:/Users/me/Downloads", &FS).unwrap(), "C:\\Users\\me\\Downloads");
        for (bad, why) in [
            ("C:\\", "whole drive"), ("C:", "whole drive"), ("D:\\", "whole drive"), ("relative", "full folder path"), ("\\\\server\\share", "full folder path"),
            ("C:\\Windows\\System32", "cannot be authorised"), ("C:\\Users\\me\\AppData\\Local\\Temp", "cannot be authorised"), ("C:\\Users\\me\\Link", "junction"),
            ("C:\\Users\\me\\Nope", "does not exist"), ("C:\\Users\\me\\Downloads\\..\\Documents", "full folder path"),
        ] {
            assert!(validate_root(bad, &FS).unwrap_err().contains(why), "{bad}");
        }
    }

    #[test]
    fn the_list_starts_empty_and_is_persisted_deduplicated_and_bounded() {
        let (_d, db) = db();
        assert!(list(&db).is_empty());
        assert_eq!(add(&db, "C:\\Users\\me\\Downloads\\Old", &FS).unwrap(), vec!["C:\\Users\\me\\Downloads\\Old"]);
        assert_eq!(add(&db, "C:\\Users\\me\\Downloads", &FS).unwrap(), vec!["C:\\Users\\me\\Downloads"], "a parent absorbs its sub-folder");
        assert!(add(&db, "C:\\Users\\me\\Downloads\\Old", &FS).unwrap_err().contains("already covered"));
        assert_eq!(add(&db, "c:\\users\\ME\\downloads", &FS).unwrap().len(), 1, "same folder, different case");
        assert_eq!(list(&db), vec!["C:\\Users\\me\\Downloads"]);
        add(&db, "C:\\Users\\me\\Documents", &FS).unwrap();
        assert_eq!(remove(&db, "C:\\Users\\me\\Documents").unwrap(), vec!["C:\\Users\\me\\Downloads"]);
        assert!(remove(&db, "C:\\nowhere").is_err());
    }
}
