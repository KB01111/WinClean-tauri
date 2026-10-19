//! Profile catalogue: the five built-in profiles plus user-defined ones,
//! the profile WinClean currently has applied, and the auto-switch settings.
//! All of it lives in `optimizer_settings`, so it survives restarts and needs
//! no schema change.

use serde::{Deserialize, Serialize};

use crate::db::Database;

use super::auto_switch::AutoSwitchConfig;
use super::policy::{builtin_profiles, validate_custom_profile, PowerProfile, MAX_CUSTOM_PROFILES};

const KEY_CUSTOM: &str = "profiles.custom";
const KEY_ACTIVE: &str = "profiles.active";
const KEY_AUTO: &str = "autoSwitch.config";
const KEY_LAST_SWITCH: &str = "autoSwitch.lastSwitchMs";

/// The profile WinClean applied most recently (and who asked for it).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActiveProfile {
    pub profile_id: String,
    pub applied_at_ms: i64,
    /// `user`, `auto` or `ai-approved`.
    pub applied_by: String,
    pub snapshot_id: i64,
}

fn read<T: for<'de> Deserialize<'de>>(db: &Database, key: &str) -> Option<T> {
    db.get_optimizer_setting(key).ok().flatten().and_then(|raw| serde_json::from_str(&raw).ok())
}

fn write<T: Serialize>(db: &Database, key: &str, value: &T) -> Result<(), String> {
    let raw = serde_json::to_string(value).map_err(|e| e.to_string())?;
    db.set_optimizer_setting(key, &raw).map_err(|e| e.to_string())
}

pub fn custom_profiles(db: &Database) -> Vec<PowerProfile> {
    read::<Vec<PowerProfile>>(db, KEY_CUSTOM).unwrap_or_default().into_iter().filter(|p| validate_custom_profile(p).is_ok()).collect()
}

/// Built-in profiles first, then custom ones.
pub fn list_profiles(db: &Database) -> Vec<PowerProfile> {
    let mut all = builtin_profiles();
    all.extend(custom_profiles(db));
    all
}

/// Looks a profile up by id or (case-insensitively) by label.
pub fn find_profile(db: &Database, id_or_label: &str) -> Option<PowerProfile> {
    list_profiles(db).into_iter().find(|p| p.id.eq_ignore_ascii_case(id_or_label) || p.label.eq_ignore_ascii_case(id_or_label))
}

/// Creates or updates a custom profile (validated; capped).
pub fn save_custom_profile(db: &Database, mut profile: PowerProfile) -> Result<PowerProfile, String> {
    profile.builtin = false;
    profile.label = profile.label.trim().to_string();
    profile.eco_qos_for_background = profile.eco_qos_mode != super::policy::EcoQosMode::Off;
    validate_custom_profile(&profile)?;
    let mut custom = custom_profiles(db);
    if let Some(existing) = custom.iter_mut().find(|p| p.id == profile.id) {
        *existing = profile.clone();
    } else {
        if custom.len() >= MAX_CUSTOM_PROFILES {
            return Err(format!("at most {MAX_CUSTOM_PROFILES} custom profiles are supported"));
        }
        if list_profiles(db).iter().any(|p| p.label.eq_ignore_ascii_case(&profile.label)) {
            return Err("a profile with that name already exists".into());
        }
        custom.push(profile.clone());
    }
    write(db, KEY_CUSTOM, &custom)?;
    Ok(profile)
}

pub fn delete_custom_profile(db: &Database, id: &str) -> Result<(), String> {
    if !id.starts_with("custom:") {
        return Err("built-in profiles cannot be deleted".into());
    }
    let mut custom = custom_profiles(db);
    let before = custom.len();
    custom.retain(|p| p.id != id);
    if custom.len() == before {
        return Err("no such custom profile".into());
    }
    write(db, KEY_CUSTOM, &custom)
}

pub fn get_active(db: &Database) -> Option<ActiveProfile> {
    read(db, KEY_ACTIVE)
}

pub fn set_active(db: &Database, active: Option<&ActiveProfile>) -> Result<(), String> {
    match active {
        Some(a) => write(db, KEY_ACTIVE, a),
        None => db.set_optimizer_setting(KEY_ACTIVE, "null").map_err(|e| e.to_string()),
    }
}

pub fn get_auto_switch(db: &Database) -> AutoSwitchConfig {
    read(db, KEY_AUTO).unwrap_or_default()
}

pub fn set_auto_switch(db: &Database, config: AutoSwitchConfig) -> Result<AutoSwitchConfig, String> {
    let config = config.validated()?;
    // Every referenced profile must exist, so a typo can never wedge the rules.
    for id in [&config.on_ac_profile, &config.on_battery_profile, &config.low_battery_profile, &config.gaming_profile, &config.ai_development_profile, &config.coding_profile] {
        if !id.trim().is_empty() && find_profile(db, id).is_none() {
            return Err(format!("unknown profile '{id}' in the automatic switching rules"));
        }
    }
    write(db, KEY_AUTO, &config)?;
    Ok(config)
}

pub fn last_auto_switch_ms(db: &Database) -> Option<i64> {
    read(db, KEY_LAST_SWITCH)
}

pub fn set_last_auto_switch_ms(db: &Database, at_ms: i64) {
    let _ = write(db, KEY_LAST_SWITCH, &at_ms);
}

/// When the user last chose a profile by hand (for the override window).
pub fn last_manual_ms(db: &Database) -> Option<i64> {
    get_active(db).filter(|a| a.applied_by == "user").map(|a| a.applied_at_ms)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optimizer::policy::{EcoQosMode, SCHEME_BALANCED};

    fn db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("t.sqlite3")).unwrap();
        db.migrate().unwrap();
        (dir, db)
    }

    fn custom(id: &str, label: &str) -> PowerProfile {
        PowerProfile { id: id.into(), label: label.into(), scheme_guid: Some(SCHEME_BALANCED.into()), eco_qos_for_background: true, eco_qos_mode: EcoQosMode::Battery, builtin: false, description: String::new() }
    }

    #[test]
    fn built_ins_come_first_and_custom_profiles_persist() {
        let (_d, db) = db();
        assert_eq!(list_profiles(&db).len(), 5);
        save_custom_profile(&db, custom("custom:travel", "Travel")).unwrap();
        let all = list_profiles(&db);
        assert_eq!(all.len(), 6);
        assert_eq!(all[5].id, "custom:travel");
        assert!(!all[5].builtin);
        assert_eq!(find_profile(&db, "travel").unwrap().id, "custom:travel", "label lookup is case-insensitive");
    }

    #[test]
    fn custom_profiles_cannot_shadow_built_ins_or_exceed_the_cap() {
        let (_d, db) = db();
        assert!(save_custom_profile(&db, custom("custom:x", "Balanced")).is_err());
        assert!(save_custom_profile(&db, custom("balanced", "Mine")).is_err());
        for i in 0..MAX_CUSTOM_PROFILES {
            save_custom_profile(&db, custom(&format!("custom:p{i}"), &format!("Profile {i}"))).unwrap();
        }
        assert!(save_custom_profile(&db, custom("custom:one-too-many", "One too many")).is_err());
        // Updating an existing one is still allowed at the cap.
        assert!(save_custom_profile(&db, custom("custom:p0", "Profile 0")).is_ok());
    }

    #[test]
    fn delete_only_removes_custom_profiles() {
        let (_d, db) = db();
        save_custom_profile(&db, custom("custom:a", "A")).unwrap();
        assert!(delete_custom_profile(&db, "balanced").is_err());
        assert!(delete_custom_profile(&db, "custom:missing").is_err());
        delete_custom_profile(&db, "custom:a").unwrap();
        assert!(custom_profiles(&db).is_empty());
    }

    #[test]
    fn auto_switch_config_must_reference_real_profiles() {
        let (_d, db) = db();
        let mut c = AutoSwitchConfig { enabled: true, ..Default::default() };
        assert!(set_auto_switch(&db, c.clone()).is_ok());
        assert!(get_auto_switch(&db).enabled);
        c.on_ac_profile = "doesNotExist".into();
        assert!(set_auto_switch(&db, c).unwrap_err().contains("unknown profile"));
        assert!(get_auto_switch(&db).enabled, "a rejected update leaves the stored config alone");
    }

    #[test]
    fn active_profile_round_trips_and_manual_time_only_counts_user_choices() {
        let (_d, db) = db();
        assert_eq!(get_active(&db), None);
        set_active(&db, Some(&ActiveProfile { profile_id: "gaming".into(), applied_at_ms: 42, applied_by: "auto".into(), snapshot_id: 1 })).unwrap();
        assert_eq!(get_active(&db).unwrap().profile_id, "gaming");
        assert_eq!(last_manual_ms(&db), None);
        set_active(&db, Some(&ActiveProfile { profile_id: "coding".into(), applied_at_ms: 99, applied_by: "user".into(), snapshot_id: 2 })).unwrap();
        assert_eq!(last_manual_ms(&db), Some(99));
        set_active(&db, None).unwrap();
        assert_eq!(get_active(&db), None);
    }
}
