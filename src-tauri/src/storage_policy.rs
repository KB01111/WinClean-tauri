//! What storage cleanup must never touch, however a candidate was found.
//!
//! One deterministic policy sits between *every* source of candidates
//! (extension rules, size/age heuristics, duplicates, the AI assistant, Windows
//! MCP search) and *every* action (recycle, offload). It protects:
//!
//! * system locations (Windows, program files, app data, the Recycle Bin, …);
//! * source-code projects — a folder holding a project marker (`.git`,
//!   `package.json`, `Cargo.toml`, `*.sln`, …) and well-known tool folders
//!   such as `node_modules`;
//! * folders synced to the cloud, where recycling would also delete the cloud copy;
//! * file types that are never "clutter" (databases, virtual disks, mail stores,
//!   key material, password vaults, office documents);
//! * any folder the user excluded.
//!
//! A model's confidence, or a file's extension, never overrides this.

use serde::Serialize;

use crate::cleanup::{is_cleanup_protected_path, is_protected_path};
use crate::db::Database;
use crate::pathutil::{is_within, norm_path};

const KEY_EXCLUSIONS: &str = "storage.excludedLocations";
pub const MAX_EXCLUSIONS: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BlockReason {
    SystemLocation,
    DevelopmentProject,
    SyncedFolder,
    ProtectedFileType,
    UserExcluded,
    /// Cloud placeholder (OneDrive-style): recycling it could delete the online copy.
    CloudPlaceholder,
    /// Junction, symlink or other reparse point.
    ReparsePoint,
    SystemFile,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Blocked {
    pub reason: BlockReason,
    pub detail: String,
}

fn blocked(reason: BlockReason, detail: impl Into<String>) -> Option<Blocked> {
    Some(Blocked { reason, detail: detail.into() })
}

/// Tool and dependency folders that are part of a development workflow.
const DEV_SEGMENTS: &[&str] = &[
    ".git", ".svn", ".hg", "node_modules", ".vscode", ".idea", ".venv", "venv", "site-packages", ".cargo", ".rustup", ".gradle", ".m2", ".nuget", ".docker", ".ssh", ".gnupg", "source/repos", "repos",
];

/// Cloud-sync client folders.
const SYNC_SEGMENTS: &[&str] = &["onedrive", "google drive", "googledrive", "dropbox", "icloud drive", "iclouddrive", "box sync", "megasync", "pcloud drive"];

/// Databases, virtual disks, mail stores, key material and vaults: never clutter.
const DATA_TYPES: &[&str] = &["vhd", "vhdx", "vmdk", "vdi", "qcow2", "pst", "ost", "mdf", "ldf", "ndf", "sqlite", "sqlite3", "db", "accdb", "mdb", "kdbx", "kdb", "key", "pem", "pfx", "p12", "ppk", "wallet"];

/// Office-style documents. Protected everywhere except where an existing rule
/// explicitly offers them (old files in Downloads, via Drive offload).
const DOCUMENT_TYPES: &[&str] = &["docx", "doc", "xlsx", "xls", "pptx", "ppt", "pdf", "odt", "ods", "odp", "rtf", "one", "pages", "numbers"];

pub fn is_document_type(extension: &str) -> bool {
    DOCUMENT_TYPES.contains(&extension.trim_start_matches('.').to_ascii_lowercase().as_str())
}

/// Names (or extensions) whose presence makes the containing folder a project.
pub const PROJECT_MARKER_NAMES: &[&str] = &[
    ".git", "package.json", "cargo.toml", "go.mod", "pyproject.toml", "setup.py", "pom.xml", "build.gradle", "build.gradle.kts", "composer.json", "gemfile", "pubspec.yaml", "cmakelists.txt", "project.godot",
];
pub const PROJECT_MARKER_EXTENSIONS: &[&str] = &["sln", "csproj", "vcxproj", "fsproj", "uproject"];

#[derive(Clone, Debug, Default)]
pub struct StoragePolicy {
    /// Normalised project root folders (outermost only).
    project_roots: Vec<String>,
    /// Normalised user exclusions.
    exclusions: Vec<String>,
}

fn has_segment(norm: &str, segment: &str) -> bool {
    let padded = format!("/{norm}/");
    padded.contains(&format!("/{segment}/"))
}

impl StoragePolicy {
    pub fn new(project_roots: Vec<String>, exclusions: Vec<String>) -> Self {
        let mut roots: Vec<String> = project_roots.iter().map(|r| norm_path(r)).filter(|r| !r.is_empty()).collect();
        roots.sort();
        roots.dedup();
        // Keep only outermost roots: a nested root adds nothing and costs lookups.
        let mut outermost: Vec<String> = Vec::new();
        for r in roots {
            if !outermost.iter().any(|o| r == *o || r.starts_with(&format!("{o}/"))) {
                outermost.push(r);
            }
        }
        Self { project_roots: outermost, exclusions: exclusions.iter().map(|e| norm_path(e)).filter(|e| !e.is_empty()).collect() }
    }

    /// Policy for one scan run: project roots come from the scan index itself.
    pub fn load(db: &Database, scan_run_id: i64) -> anyhow::Result<Self> {
        let roots = db.project_marker_dirs(scan_run_id)?;
        Ok(Self::new(roots, exclusions(db)))
    }

    /// Re-read the user's exclusions (they can change while a long job runs); project roots are kept.
    pub fn refresh_exclusions(&mut self, db: &Database) {
        self.exclusions = exclusions(db).iter().map(|e| norm_path(e)).filter(|e| !e.is_empty()).collect();
    }

    pub fn project_root_count(&self) -> usize {
        self.project_roots.len()
    }

    /// Why `path` must not be a cleanup candidate, if it must not.
    /// Like [`check_for_cleanup`], also honouring the attributes recorded by the scan.
    pub fn check_entry(&self, entry: &crate::db::FileSearchResult, allow_documents: bool) -> Option<Blocked> {
        if entry.is_reparse_point {
            return blocked(BlockReason::ReparsePoint, "A link or junction, not a regular file");
        }
        if entry.is_placeholder {
            return blocked(BlockReason::CloudPlaceholder, "A cloud placeholder; the real content lives online");
        }
        if entry.is_system {
            return blocked(BlockReason::SystemFile, "Marked as a system file");
        }
        self.check_for_cleanup(&entry.path, entry.extension.as_deref(), allow_documents)
    }

    /// Cleanup view: a user's Temp and crash-dump folders may be cleaned (their age is checked elsewhere).
    pub fn check_for_cleanup(&self, path: &str, extension: Option<&str>, allow_documents: bool) -> Option<Blocked> {
        self.check_inner(path, extension, allow_documents, true)
    }

    /// Strict view, for anything that would *read* a file (duplicate comparison): every
    /// application-data location, Temp included, is off limits.
    pub fn check_with(&self, path: &str, extension: Option<&str>, allow_documents: bool) -> Option<Blocked> {
        self.check_inner(path, extension, allow_documents, false)
    }

    pub fn check(&self, path: &str, extension: Option<&str>) -> Option<Blocked> {
        self.check_with(path, extension, false)
    }

    fn check_inner(&self, path: &str, extension: Option<&str>, allow_documents: bool, allow_scratch: bool) -> Option<Blocked> {
        let protected = if allow_scratch { is_cleanup_protected_path(path) } else { is_protected_path(path) };
        if protected {
            return blocked(BlockReason::SystemLocation, "System, program, application-data or Recycle Bin location");
        }
        let norm = norm_path(path);
        if let Some(ex) = self.exclusions.iter().find(|e| norm == **e || norm.starts_with(&format!("{e}/"))) {
            return blocked(BlockReason::UserExcluded, format!("Inside a location you excluded ({ex})"));
        }
        if let Some(seg) = DEV_SEGMENTS.iter().find(|s| has_segment(&norm, s)) {
            return blocked(BlockReason::DevelopmentProject, format!("Inside a development folder ({seg})"));
        }
        if self.project_roots.iter().any(|r| norm.starts_with(&format!("{r}/"))) {
            return blocked(BlockReason::DevelopmentProject, "Inside a source-code project (a project file such as .git or package.json is in this folder tree)");
        }
        if let Some(seg) = SYNC_SEGMENTS.iter().find(|s| has_segment(&norm, s)) {
            return blocked(BlockReason::SyncedFolder, format!("Inside a cloud-synced folder ({seg}); recycling here would also delete the cloud copy"));
        }
        let ext = extension.map(|e| e.trim_start_matches('.').to_ascii_lowercase()).or_else(|| norm.rsplit_once('.').map(|(_, e)| e.to_string())).unwrap_or_default();
        if DATA_TYPES.contains(&ext.as_str()) || (!allow_documents && DOCUMENT_TYPES.contains(&ext.as_str())) {
            return blocked(BlockReason::ProtectedFileType, format!("A .{ext} file holds user data (database, virtual disk, mail, key or document)"));
        }
        None
    }
}

fn read_exclusions(db: &Database) -> Vec<String> {
    db.get_optimizer_setting(KEY_EXCLUSIONS).ok().flatten().and_then(|raw| serde_json::from_str::<Vec<String>>(&raw).ok()).unwrap_or_default()
}

pub fn exclusions(db: &Database) -> Vec<String> {
    read_exclusions(db)
}

/// Adds a folder the cleanup must never touch (the folder need not exist).
pub fn add_exclusion(db: &Database, path: &str) -> Result<Vec<String>, String> {
    let trimmed = path.trim().trim_matches('"').trim_end_matches(['\\', '/']).replace('/', "\\");
    let b = trimmed.as_bytes();
    let drive = b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && b[2] == b'\\';
    if !drive || trimmed.chars().any(|c| c.is_control()) || trimmed.split('\\').any(|s| s == "..") || trimmed.len() > 1024 {
        return Err("enter a full folder path such as D:\\Projects".into());
    }
    let mut list = read_exclusions(db);
    if list.iter().any(|e| norm_path(e) == norm_path(&trimmed)) {
        return Ok(list);
    }
    if list.len() >= MAX_EXCLUSIONS {
        return Err(format!("at most {MAX_EXCLUSIONS} locations can be excluded"));
    }
    list.retain(|e| !is_within(&trimmed, e));
    list.push(trimmed);
    db.set_optimizer_setting(KEY_EXCLUSIONS, &serde_json::to_string(&list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(list)
}

pub fn remove_exclusion(db: &Database, path: &str) -> Result<Vec<String>, String> {
    let mut list = read_exclusions(db);
    let before = list.len();
    list.retain(|e| norm_path(e) != norm_path(path.trim()));
    if list.len() == before {
        return Err("that location is not in the list".into());
    }
    db.set_optimizer_setting(KEY_EXCLUSIONS, &serde_json::to_string(&list).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(list)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> StoragePolicy {
        StoragePolicy::new(vec!["C:\\Users\\me\\code\\app".into(), "C:\\Users\\me\\code\\app\\packages\\inner".into(), "D:\\work\\tool".into()], vec!["D:\\Photos\\Wedding".into()])
    }

    #[test]
    fn ordinary_clutter_is_not_blocked() {
        let p = policy();
        for (path, ext) in [("C:\\Users\\me\\Downloads\\setup.msi", Some("msi")), ("C:\\Users\\me\\Downloads\\old.zip", None), ("D:\\temp\\cache.tmp", Some("tmp")), ("C:\\Users\\me\\Videos\\clip.mp4", None)] {
            assert_eq!(p.check(path, ext), None, "{path}");
        }
    }

    #[test]
    fn system_locations_include_the_recycle_bin_and_windows_old() {
        let p = StoragePolicy::default();
        for path in [
            "C:\\Windows\\Temp\\a.log", "C:\\$Recycle.Bin\\S-1-5-21-1\\$R1234.log", "C:\\Windows.old\\Users\\me\\a.tmp", "C:\\System Volume Information\\x.dat", "C:\\Program Files\\App\\a.log",
            "C:\\ProgramData\\x\\a.tmp", "C:\\Users\\me\\AppData\\Roaming\\x\\a.tmp", "\\\\?\\C:\\Windows\\a.log", "C:\\$WINDOWS.~BT\\Sources\\a.log", "C:\\Recovery\\a.wim", "D:\\$Extend\\x", "C:\\a.log",
        ] {
            assert_eq!(p.check(path, None).map(|b| b.reason), Some(BlockReason::SystemLocation), "{path}");
        }
    }

    #[test]
    fn only_the_user_temp_and_crash_dump_folders_are_opened_up_inside_appdata() {
        let p = StoragePolicy::default();
        for path in ["C:\\Users\\me\\AppData\\Local\\Temp\\a.tmp", "D:\\Users\\me\\AppData\\Local\\Temp\\sub\\b.log", "C:\\Users\\me\\AppData\\Local\\CrashDumps\\x.dmp"] {
            assert_eq!(p.check_for_cleanup(path, None, false), None, "{path}");
            assert_eq!(p.check_with(path, None, true).map(|b| b.reason), Some(BlockReason::SystemLocation), "reading (duplicate comparison) stays off limits: {path}");
        }
        for path in [
            "C:\\Users\\me\\AppData\\Local\\Temp",
            "C:\\Users\\me\\AppData\\Local\\Temporary\\a.tmp",
            "C:\\Users\\me\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\a.tmp",
            "C:\\Users\\me\\AppData\\Local\\Packages\\x\\a.tmp",
            "C:\\Users\\me\\AppData\\Roaming\\Temp\\a.tmp",
            "C:\\Other\\AppData\\Local\\Temp\\a.tmp",
            "C:\\Users\\me\\AppData\\Local\\Temp\\Windows\\a.tmp",
        ] {
            assert_eq!(p.check_for_cleanup(path, None, false).map(|b| b.reason), Some(BlockReason::SystemLocation), "{path}");
        }
    }

    #[test]
    fn an_exclusion_added_later_applies_after_a_refresh() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite3")).unwrap();
        db.migrate().unwrap();
        let mut p = StoragePolicy::default();
        assert_eq!(p.check_with("D:\\Photos\\a.png", None, true), None);
        add_exclusion(&db, "D:\\Photos").unwrap();
        assert_eq!(p.check_with("D:\\Photos\\a.png", None, true), None, "a loaded policy is a snapshot");
        p.refresh_exclusions(&db);
        assert_eq!(p.check_with("D:\\Photos\\a.png", None, true).map(|b| b.reason), Some(BlockReason::UserExcluded));
    }

    #[test]
    fn source_code_projects_are_protected_by_marker_roots_and_well_known_folders() {
        let p = policy();
        for path in [
            "C:\\Users\\me\\code\\app\\target\\debug\\build.log", "C:\\Users\\me\\code\\app\\packages\\inner\\x.tmp", "C:\\Users\\me\\code\\other\\node_modules\\pkg\\big.bin", "E:\\x\\.git\\objects\\pack\\p.pack",
            "D:\\work\\tool\\old.bak", "C:\\Users\\me\\source\\repos\\thing\\x.log", "C:\\Users\\me\\.cargo\\registry\\a.crate", "C:\\Users\\me\\.ssh\\known_hosts.old",
        ] {
            assert_eq!(p.check(path, None).map(|b| b.reason), Some(BlockReason::DevelopmentProject), "{path}");
        }
        assert_eq!(p.project_root_count(), 2, "the nested root collapses into its parent");
        assert_eq!(p.check("C:\\Users\\me\\code\\application2\\a.tmp", None), None, "a sibling with a similar name is not inside the project");
        assert_eq!(p.check("C:\\Users\\me\\code\\app", None).map(|b| b.reason), None, "only things inside the root are covered");
    }

    #[test]
    fn user_exclusions_and_synced_folders_and_user_data_types_are_protected() {
        let p = policy();
        assert_eq!(p.check("D:\\Photos\\Wedding\\IMG1.tmp", None).map(|b| b.reason), Some(BlockReason::UserExcluded));
        assert_eq!(p.check("d:/photos/wedding", None).map(|b| b.reason), Some(BlockReason::UserExcluded));
        assert_eq!(p.check("D:\\Photos\\Wedding2\\a.tmp", None), None);
        for path in ["C:\\Users\\me\\OneDrive\\Docs\\big.iso", "C:\\Users\\me\\Google Drive\\x.zip", "C:\\Users\\me\\Dropbox\\y.mp4"] {
            assert_eq!(p.check(path, None).map(|b| b.reason), Some(BlockReason::SyncedFolder), "{path}");
        }
        for path in ["C:\\Users\\me\\Downloads\\mail.pst", "D:\\vm\\disk.vhdx", "D:\\data\\app.sqlite", "C:\\Users\\me\\Downloads\\contract.pdf", "D:\\k\\vault.kdbx"] {
            assert_eq!(p.check(path, None).map(|b| b.reason), Some(BlockReason::ProtectedFileType), "{path}");
        }
        assert_eq!(p.check("C:\\Users\\me\\Downloads\\a.pdf.crdownload", None), None);
        assert_eq!(p.check_with("C:\\Users\\me\\Downloads\\invoice.pdf", None, true), None, "documents may pass where an existing rule offers them");
        assert!(p.check_with("D:\\data\\app.sqlite", None, true).is_some(), "databases never pass");
        assert!(is_document_type(".PDF") && !is_document_type("iso"));
    }

    #[test]
    fn exclusions_are_persisted_validated_and_bounded() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite3")).unwrap();
        db.migrate().unwrap();
        assert!(exclusions(&db).is_empty());
        assert_eq!(add_exclusion(&db, "\"D:\\Projects\\\"").unwrap(), vec!["D:\\Projects"]);
        assert_eq!(add_exclusion(&db, "d:/projects").unwrap().len(), 1);
        assert_eq!(add_exclusion(&db, "D:\\Projects\\Sub").unwrap().len(), 2, "a narrower folder is kept (harmless)");
        for bad in ["", "relative", "D:", "D:\\a\\..\\b", "\\\\server\\share", "D:\\a\u{0}b"] {
            assert!(add_exclusion(&db, bad).is_err(), "{bad:?}");
        }
        assert_eq!(remove_exclusion(&db, "D:\\Projects\\Sub").unwrap(), vec!["D:\\Projects"]);
        assert!(remove_exclusion(&db, "D:\\nope").is_err());
        assert_eq!(StoragePolicy::load(&db, 1).unwrap().check("D:\\Projects\\a.tmp", None).map(|b| b.reason), Some(BlockReason::UserExcluded));
    }
}
