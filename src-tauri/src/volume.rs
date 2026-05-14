use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeInfo {
    pub name: String,
    pub root_path: String,
    pub file_system: Option<String>,
    pub drive_type: String,
    pub is_ready: bool,
    pub available_space: Option<u64>,
    pub total_space: Option<u64>,
}

pub fn list_volumes() -> Vec<VolumeInfo> {
    platform_volumes()
}

pub fn resolve_local_scan_root(
    root_path: &str,
    volumes: &[VolumeInfo],
) -> Result<VolumeInfo, String> {
    let requested = normalize_volume_root(root_path);
    let Some(volume) = volumes
        .iter()
        .find(|volume| normalize_volume_root(&volume.root_path) == requested)
    else {
        return Err("Select a local volume root from the available drives.".to_string());
    };

    if !volume.is_ready {
        return Err(format!("{} is not ready for scanning.", volume.root_path));
    }

    if !matches!(volume.drive_type.as_str(), "fixed" | "removable") {
        return Err(format!(
            "{} is not a supported local fixed or removable drive.",
            volume.root_path
        ));
    }

    Ok(volume.clone())
}

fn normalize_volume_root(path: &str) -> String {
    let mut normalized = path.trim().replace('/', "\\").to_ascii_lowercase();
    if normalized.len() == 2 && normalized.ends_with(':') {
        normalized.push('\\');
    }
    normalized
}

#[cfg(windows)]
fn platform_volumes() -> Vec<VolumeInfo> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives};

    let mut volumes = Vec::new();
    let mask = unsafe { GetLogicalDrives() };

    for index in 0..26 {
        if mask & (1 << index) == 0 {
            continue;
        }

        let letter = (b'A' + index as u8) as char;
        let root_path = format!("{letter}:\\");
        let wide = wide_null(&root_path);
        let drive_type_code = unsafe { GetDriveTypeW(wide.as_ptr()) };
        let (file_system, is_ready) = volume_file_system(wide.as_ptr());
        let (available_space, total_space) = disk_space(wide.as_ptr());

        volumes.push(VolumeInfo {
            name: root_path.clone(),
            root_path,
            file_system,
            drive_type: match drive_type_code {
                2 => "removable",
                3 => "fixed",
                4 => "remote",
                5 => "cdrom",
                6 => "ramdisk",
                _ => "unknown",
            }
            .to_string(),
            is_ready,
            available_space,
            total_space,
        });
    }

    volumes
}

#[cfg(windows)]
fn volume_file_system(root: *const u16) -> (Option<String>, bool) {
    use windows_sys::Win32::Storage::FileSystem::GetVolumeInformationW;

    let mut fs_name = [0_u16; 64];
    let ok = unsafe {
        GetVolumeInformationW(
            root,
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            fs_name.as_mut_ptr(),
            fs_name.len() as u32,
        )
    };

    if ok == 0 {
        return (None, false);
    }

    let len = fs_name
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(fs_name.len());
    (Some(String::from_utf16_lossy(&fs_name[..len])), true)
}

#[cfg(windows)]
fn disk_space(root: *const u16) -> (Option<u64>, Option<u64>) {
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut available = 0_u64;
    let mut total = 0_u64;
    let mut free = 0_u64;
    let ok = unsafe { GetDiskFreeSpaceExW(root, &mut available, &mut total, &mut free) };

    if ok == 0 {
        return (None, None);
    }

    (Some(available), Some(total))
}

#[cfg(windows)]
fn wide_null(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    std::ffi::OsStr::new(value)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(not(windows))]
fn platform_volumes() -> Vec<VolumeInfo> {
    vec![VolumeInfo {
        name: "/".to_string(),
        root_path: "/".to_string(),
        file_system: None,
        drive_type: "root".to_string(),
        is_ready: true,
        available_space: None,
        total_space: None,
    }]
}
