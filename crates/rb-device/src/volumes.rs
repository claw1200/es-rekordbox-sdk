//! Mounted volumes, and ejecting or formatting a removable one.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::paths::{has_library, ROOT_NAMES};
use crate::{fail, Result};

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

#[cfg(any(windows, test))]
mod safe_removal;

#[cfg(not(any(target_os = "macos", windows)))]
mod unsupported;
#[cfg(not(any(target_os = "macos", windows)))]
use unsupported as platform;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Volume {
    pub id: String,
    pub name: String,
    pub path: String,
    pub has_library: bool,
    pub ejectable: bool,
    pub root_name: Option<String>,
    pub file_system: String,
    pub bytes_total: u64,
    pub bytes_used: u64,
    pub bytes_free: u64,
}

/// A volume as the operating system describes it, before anything on it is read.
struct Mounted {
    path: PathBuf,
    name: String,
    file_system: String,
    ejectable: bool,
}

impl From<Mounted> for Volume {
    fn from(mounted: Mounted) -> Self {
        let (bytes_total, bytes_used, bytes_free) = usage(&mounted.path);
        let path = mounted.path.to_string_lossy().into_owned();
        Self {
            id: path.clone(),
            name: mounted.name,
            path,
            has_library: has_library(&mounted.path),
            ejectable: mounted.ejectable,
            root_name: root_name(&mounted.path),
            file_system: mounted.file_system,
            bytes_total,
            bytes_used,
            bytes_free,
        }
    }
}

fn usage(path: &Path) -> (u64, u64, u64) {
    let (Ok(total), Ok(free)) = (fs2::total_space(path), fs2::free_space(path)) else { return (0, 0, 0) };
    (total, total.saturating_sub(free), free)
}

fn root_name(volume: &Path) -> Option<String> {
    ROOT_NAMES.iter().find(|name| volume.join(name).join("rekordbox").is_dir()).map(|n| (*n).to_owned())
}

fn folders_in(root: &Path) -> Vec<Mounted> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| Mounted {
            name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            file_system: String::new(),
            ejectable: false,
            path,
        })
        .collect()
}

/// Mounted volumes worth offering: the system volume never, and otherwise only those that can be
/// ejected or already hold a library. A test folder passed as `root` lists every directory in it.
#[must_use]
pub fn list(root: Option<&Path>) -> Vec<Volume> {
    let mut volumes: Vec<Volume> = match root {
        Some(root) => folders_in(root).into_iter().map(Volume::from).collect(),
        None => platform::mounted()
            .into_iter()
            .map(Volume::from)
            .filter(|volume| volume.ejectable || volume.has_library)
            .collect(),
    };
    volumes.sort_by_key(|volume| volume.name.to_lowercase());
    volumes
}

fn listed<'a>(volume: &Path, volumes: &'a [Volume]) -> Option<&'a Volume> {
    let wanted = volume.canonicalize().ok()?;
    volumes.iter().find(|v| Path::new(&v.path).canonicalize().ok().as_ref() == Some(&wanted))
}

fn listed_ejectable(volume_id: &str, volumes: Option<&[Volume]>) -> Option<Volume> {
    let current;
    let volumes = match volumes {
        Some(v) => v,
        None => {
            current = list(None);
            &current
        }
    };
    listed(Path::new(volume_id), volumes).filter(|v| v.ejectable).cloned()
}

/// Eject a removable volume that appears in the device list.
pub fn eject(volume_id: &str, volumes: Option<&[Volume]>) -> Result<()> {
    let Some(volume) = listed_ejectable(volume_id, volumes) else {
        return fail("That device can’t be ejected");
    };
    platform::eject(Path::new(&volume.path))
}

/// A FAT32 volume label: capitals and digits, at most 11 characters.
#[must_use]
pub fn fat_label(name: &str) -> String {
    let cleaned: String = name.to_uppercase().chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
    let trimmed = cleaned.trim_matches('_');
    let label = if trimmed.is_empty() { "USB" } else { trimmed };
    label.chars().take(11).collect()
}

/// Erase a removable volume and format it FAT32.
pub fn format(volume_id: &str, volumes: Option<&[Volume]>) -> Result<()> {
    let Some(volume) = listed_ejectable(volume_id, volumes) else {
        return fail("That device can’t be formatted");
    };
    platform::erase_as_fat32(Path::new(&volume.path), &fat_label(&volume.name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_fat_safe() {
        assert_eq!(fat_label("my stick!"), "MY_STICK");
        assert_eq!(fat_label("___"), "USB");
        assert_eq!(fat_label("A very long volume name"), "A_VERY_LONG");
    }

    #[test]
    fn a_test_folder_lists_every_directory_with_its_library_flag() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Plain")).unwrap();
        std::fs::create_dir_all(dir.path().join("Stick/PIONEER/rekordbox")).unwrap();
        std::fs::write(dir.path().join("Stick/PIONEER/rekordbox/export.pdb"), b"x").unwrap();
        let found = list(Some(dir.path()));
        let summary: Vec<_> = found.iter().map(|v| (v.name.as_str(), v.has_library, v.root_name.as_deref())).collect();
        assert_eq!(summary, [("Plain", false, None), ("Stick", true, Some("PIONEER"))]);
    }

    #[test]
    fn only_listed_ejectable_volumes_can_be_ejected() {
        let dir = tempfile::tempdir().unwrap();
        let volume = Volume {
            id: dir.path().to_string_lossy().into_owned(),
            name: "x".into(),
            path: dir.path().to_string_lossy().into_owned(),
            has_library: false,
            ejectable: false,
            root_name: None,
            file_system: String::new(),
            bytes_total: 0,
            bytes_used: 0,
            bytes_free: 0,
        };
        assert!(eject(&volume.id, Some(std::slice::from_ref(&volume))).is_err());
        assert!(eject("/nowhere", Some(&[volume])).is_err());
    }

    #[test]
    fn the_real_list_never_offers_a_volume_it_could_neither_eject_nor_read() {
        assert!(list(None).iter().all(|volume| volume.ejectable || volume.has_library));
    }
}
