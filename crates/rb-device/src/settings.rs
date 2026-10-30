//! Per-device display and browse settings, ported from `rbl-devices`.
//!
//! Display choices live in `DEVSETTING.DAT`; browse categories, sort options,
//! the sub-column, device name and color labels live in `exportLibrary.db`.
//! Name, labels and the Device Library background are also stored in `export.pdb`.
//! Reads never recover or modify a volume. Writes require prior export recovery,
//! serialize with exports via the storage lease, and publish changed files together.

use std::path::{Path, PathBuf};

use rb_storage::durable::{self, Publication};
pub use rbl_onelibrary::settings::{ColorName, MenuSlot, StickSettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaveformColor {
    #[default]
    Blue,
    Rgb,
    TriBand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WaveformPosition {
    #[default]
    Center,
    Left,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverviewWaveform {
    #[default]
    Half,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeyDisplay {
    #[default]
    Classic,
    Alphanumeric,
}

/// The four understood display fields. All other bytes are preserved on write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevSetting {
    pub overview: OverviewWaveform,
    pub color: WaveformColor,
    pub key_display: KeyDisplay,
    pub position: WaveformPosition,
    file: [u8; 140],
}

const BODY_AT: usize = 0x68;
const CHECKSUM_AT: usize = 0x88;
const JOURNAL: &str = ".exportstudio-publication";

impl Default for DevSetting {
    fn default() -> Self {
        let mut file = [0; 140];
        file[..4].copy_from_slice(&0x60_u32.to_le_bytes());
        for (offset, text) in [(4, "PIONEER DJ"), (0x24, "rekordbox"), (0x44, "7.2.11")] {
            file[offset..offset + text.len()].copy_from_slice(text.as_bytes());
        }
        file[0x64..BODY_AT].copy_from_slice(&32_u32.to_le_bytes());
        file[BODY_AT..BODY_AT + 14]
            .copy_from_slice(&[0x78, 0x56, 0x34, 0x12, 1, 0, 0, 0, 1, 1, 1, 1, 1, 1]);
        let checksum = crc16(&file[BODY_AT..CHECKSUM_AT]);
        file[CHECKSUM_AT..CHECKSUM_AT + 2].copy_from_slice(&checksum.to_le_bytes());
        Self {
            overview: OverviewWaveform::Half,
            color: WaveformColor::Blue,
            key_display: KeyDisplay::Classic,
            position: WaveformPosition::Center,
            file,
        }
    }
}

fn crc16(bytes: &[u8]) -> u16 {
    let mut crc = 0_u16;
    for byte in bytes {
        crc ^= u16::from(*byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 == 0 {
                crc << 1
            } else {
                (crc << 1) ^ 0x1021
            };
        }
    }
    crc
}

impl DevSetting {
    /// Rejects damaged files and unsupported display values instead of normalizing them.
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        let file: [u8; 140] = bytes.try_into().ok()?;
        if file[..4] != 0x60_u32.to_le_bytes()
            || file[0x64..BODY_AT] != 32_u32.to_le_bytes()
            || u16::from_le_bytes([file[0x88], file[0x89]]) != crc16(&file[BODY_AT..CHECKSUM_AT])
        {
            return None;
        }
        Some(Self {
            overview: match file[BODY_AT + 9] {
                1 => OverviewWaveform::Half,
                2 => OverviewWaveform::Full,
                _ => return None,
            },
            color: match file[BODY_AT + 10] {
                1 => WaveformColor::Blue,
                3 => WaveformColor::Rgb,
                4 => WaveformColor::TriBand,
                _ => return None,
            },
            key_display: match file[BODY_AT + 12] {
                1 => KeyDisplay::Classic,
                2 => KeyDisplay::Alphanumeric,
                _ => return None,
            },
            position: match file[BODY_AT + 13] {
                1 => WaveformPosition::Center,
                2 => WaveformPosition::Left,
                _ => return None,
            },
            file,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut file = self.file;
        file[BODY_AT + 9] = match self.overview {
            OverviewWaveform::Half => 1,
            OverviewWaveform::Full => 2,
        };
        file[BODY_AT + 10] = match self.color {
            WaveformColor::Blue => 1,
            WaveformColor::Rgb => 3,
            WaveformColor::TriBand => 4,
        };
        file[BODY_AT + 12] = match self.key_display {
            KeyDisplay::Classic => 1,
            KeyDisplay::Alphanumeric => 2,
        };
        file[BODY_AT + 13] = match self.position {
            WaveformPosition::Center => 1,
            WaveformPosition::Left => 2,
        };
        let checksum = crc16(&file[BODY_AT..CHECKSUM_AT]);
        file[CHECKSUM_AT..CHECKSUM_AT + 2].copy_from_slice(&checksum.to_le_bytes());
        file.to_vec()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceSettings {
    pub dev: Option<DevSetting>,
    pub library: Option<StickSettings>,
    pub device_library_background: Option<u8>,
    pub has_device_library: bool,
    pub has_one_library: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("could not access exportLibrary.db: {0}")]
    Library(#[from] rbl_onelibrary::Error),
    #[error("{0}")]
    Invalid(String),
}

/// Uses the same visible/hidden root selection as device library reads.
pub fn export_root(mount_point: &Path) -> PathBuf {
    crate::paths::pioneer_dir(mount_point).0
}

fn require_recovered(mount_point: &Path) -> Result<(), SettingsError> {
    if !mount_point.is_dir() {
        return Err(SettingsError::Invalid("Device is not mounted".into()));
    }
    if mount_point.join(JOURNAL).try_exists()? {
        return Err(SettingsError::Invalid(
            "Device recovery is required before accessing settings".into(),
        ));
    }
    Ok(())
}

/// Read-only, under the same shared lease used by device/export readers.
/// Missing groups are `None`; unreadable or unsupported files are errors.
pub fn read(mount_point: &Path) -> Result<DeviceSettings, SettingsError> {
    let _lease = durable::read_lease(mount_point)?;
    require_recovered(mount_point)?;
    read_files(mount_point)
}

fn read_files(mount_point: &Path) -> Result<DeviceSettings, SettingsError> {
    let root = export_root(mount_point);
    for path in [
        root.clone(),
        root.join("rekordbox"),
        root.join("DEVSETTING.DAT"),
        root.join("rekordbox/export.pdb"),
        root.join("rekordbox/exportLibrary.db"),
        root.join("rekordbox/exportLibrary.db-wal"),
        root.join("rekordbox/exportLibrary.db-shm"),
    ] {
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(SettingsError::Invalid(format!(
                    "Settings path is a symbolic link: {}",
                    path.display()
                )));
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    let dev = match std::fs::read(root.join("DEVSETTING.DAT")) {
        Ok(bytes) => Some(DevSetting::parse(&bytes).ok_or_else(|| {
            SettingsError::Invalid(
                "Unsupported or damaged DEVSETTING.DAT; it has not been changed".into(),
            )
        })?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let library_path = root.join("rekordbox/exportLibrary.db");
    let has_one_library = library_path.try_exists()?;
    let library = if has_one_library {
        Some(StickSettings::read(&library_path)?)
    } else {
        None
    };
    let pdb_path = root.join("rekordbox/export.pdb");
    let has_device_library = pdb_path.try_exists()?;
    let device_library_background = if has_device_library {
        let bytes = std::fs::read(pdb_path)?;
        let pdb = rbl_pdb::Pdb::parse(&bytes)
            .map_err(|error| SettingsError::Invalid(error.to_string()))?;
        pdb.property().map(|property| property.background_color)
    } else {
        None
    };
    Ok(DeviceSettings {
        dev,
        library,
        device_library_background,
        has_device_library,
        has_one_library,
    })
}

/// Updates only changed groups. Does not create either device library.
pub fn write(mount_point: &Path, settings: &DeviceSettings) -> Result<(), SettingsError> {
    write_inner(mount_point, None, settings)
}

/// Optimistic update: refuses a stale snapshot rather than overwriting another editor.
pub fn write_changes(
    mount_point: &Path,
    current: &DeviceSettings,
    settings: &DeviceSettings,
) -> Result<(), SettingsError> {
    write_inner(mount_point, Some(current), settings)
}

fn write_inner(
    mount_point: &Path,
    expected: Option<&DeviceSettings>,
    settings: &DeviceSettings,
) -> Result<(), SettingsError> {
    require_recovered(mount_point)?;
    let publication = Publication::new(mount_point, JOURNAL)?;
    require_recovered(mount_point)?;
    let current = read_files(mount_point)?;
    if expected.is_some_and(|expected| *expected != current) {
        return Err(SettingsError::Invalid(
            "Device settings changed; reload them before saving".into(),
        ));
    }
    let dev_changed = settings.dev.is_some() && settings.dev != current.dev;
    let library_changed = settings.library.is_some() && settings.library != current.library;
    let background_changed = settings.device_library_background.is_some()
        && settings.device_library_background != current.device_library_background;
    if !dev_changed && !library_changed && !background_changed {
        return Ok(());
    }
    if settings
        .device_library_background
        .is_some_and(|color| color > 8)
        || settings
            .library
            .as_ref()
            .is_some_and(|library| !(0..=8).contains(&library.background_color_type))
    {
        return Err(SettingsError::Invalid(
            "Background color must be between 0 and 8".into(),
        ));
    }
    let root = export_root(mount_point);
    let relative = root
        .strip_prefix(mount_point)
        .map_err(std::io::Error::other)?;
    let staged_root = publication.stage().join(relative);
    durable::create_dir_all(staged_root.join("rekordbox"))?;
    let mut files = Vec::new();
    if dev_changed {
        if let Some(dev) = &settings.dev {
            durable::write(&staged_root.join("DEVSETTING.DAT"), &dev.encode())?;
            files.push(relative.join("DEVSETTING.DAT"));
        }
    }
    if library_changed {
        if !current.has_one_library {
            return Err(SettingsError::Invalid(
                "Browse settings require an existing OneLibrary".into(),
            ));
        }
        if let Some(library) = &settings.library {
            let staged = staged_root.join("rekordbox/exportLibrary.db");
            std::fs::copy(root.join("rekordbox/exportLibrary.db"), &staged)?;
            let wal = root.join("rekordbox/exportLibrary.db-wal");
            if wal.try_exists()? {
                std::fs::copy(wal, staged_root.join("rekordbox/exportLibrary.db-wal"))?;
            }
            library.write(&staged)?;
            files.extend([
                relative.join("rekordbox/exportLibrary.db-wal"),
                relative.join("rekordbox/exportLibrary.db-shm"),
                relative.join("rekordbox/exportLibrary.db"),
            ]);
        }
    }
    if library_changed || background_changed {
        if current.has_device_library {
            let bytes = std::fs::read(root.join("rekordbox/export.pdb"))?;
            let next = updated_pdb(
                &bytes,
                if library_changed {
                    settings.library.as_ref()
                } else {
                    None
                },
                settings.device_library_background,
            )?;
            if next != bytes {
                durable::write(&staged_root.join("rekordbox/export.pdb"), &next)?;
                files.push(relative.join("rekordbox/export.pdb"));
            }
        } else if background_changed {
            return Err(SettingsError::Invalid(
                "Device Library background requires an existing export.pdb".into(),
            ));
        }
    }
    publication.commit(&files)?;
    Ok(())
}

fn updated_pdb(
    bytes: &[u8],
    library: Option<&StickSettings>,
    background: Option<u8>,
) -> Result<Vec<u8>, SettingsError> {
    let mut next = bytes.to_vec();
    if let Some(library) = library {
        let rows: Vec<_> = library
            .colors
            .iter()
            .map(|color| {
                let id = u16::try_from(color.id)
                    .map_err(|_| SettingsError::Invalid("Invalid color id".into()))?;
                Ok(rbl_pdb::rows::color_row(id, &color.name))
            })
            .collect::<Result<_, SettingsError>>()?;
        next = rbl_pdb::build::replace_single_page_table(&next, 6, &rows).ok_or_else(|| {
            SettingsError::Invalid("Cannot update export.pdb color labels".into())
        })?;
    }
    let pdb =
        rbl_pdb::Pdb::parse(&next).map_err(|error| SettingsError::Invalid(error.to_string()))?;
    if let Some(current) = pdb.property() {
        let mut property = current.clone();
        if let Some(library) = library {
            property.device_name.clone_from(&library.device_name);
        }
        if let Some(background) = background {
            property.background_color = background;
        }
        if property != current {
            let row = rbl_pdb::rows::property_row(&property)
                .ok_or_else(|| SettingsError::Invalid("Invalid Device Library property".into()))?;
            next =
                rbl_pdb::build::replace_single_page_table(&next, 19, &[row]).ok_or_else(|| {
                    SettingsError::Invalid("Cannot update Device Library property".into())
                })?;
        }
    } else if background.is_some() {
        return Err(SettingsError::Invalid(
            "Device Library has no readable property row".into(),
        ));
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_rekordbox_display_file_round_trips() {
        let bytes: [u8; 140] = [
            0x60, 0x00, 0x00, 0x00, 0x50, 0x49, 0x4f, 0x4e, 0x45, 0x45, 0x52, 0x20, 0x44, 0x4a,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x72, 0x65, 0x6b, 0x6f, 0x72, 0x64,
            0x62, 0x6f, 0x78, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x37, 0x2e,
            0x32, 0x2e, 0x38, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x20, 0x00, 0x00, 0x00, 0x78, 0x56, 0x34, 0x12, 0x01, 0x00, 0x00, 0x00,
            0x01, 0x01, 0x04, 0x01, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x8f, 0x1a, 0x00, 0x00,
        ];
        let display = DevSetting::parse(&bytes).unwrap();
        assert_eq!(display.color, WaveformColor::TriBand);
        assert_eq!(display.encode(), bytes.to_vec());
    }

    #[test]
    fn display_round_trip_preserves_unknown_bytes() {
        assert_eq!(crc16(b"123456789"), 0x31c3);
        let mut original = DevSetting::default();
        original.file[0x20] = 77;
        original.file[BODY_AT + 18] = 91;
        original.file[139] = 42;
        let bytes = original.encode();
        let mut parsed = DevSetting::parse(&bytes).unwrap();
        assert_eq!(parsed.encode(), bytes);
        parsed.key_display = KeyDisplay::Alphanumeric;
        let next = parsed.encode();
        let differences: Vec<_> = (0..140)
            .filter(|index| bytes[*index] != next[*index])
            .collect();
        assert!(differences.contains(&(BODY_AT + 12)));
        assert!(differences
            .iter()
            .all(|index| [BODY_AT + 12, 0x88, 0x89].contains(index)));
        let mut damaged = bytes;
        damaged[110] ^= 1;
        assert!(DevSetting::parse(&damaged).is_none());
    }

    #[test]
    fn display_only_write_and_stale_snapshot() {
        let volume = tempfile::tempdir().unwrap();
        let current = read(volume.path()).unwrap();
        let mut next = current.clone();
        next.dev = Some(DevSetting::default());
        write_changes(volume.path(), &current, &next).unwrap();
        assert_eq!(read(volume.path()).unwrap().dev, next.dev);
        assert!(!volume
            .path()
            .join("PIONEER/rekordbox/exportLibrary.db")
            .exists());
        assert!(write_changes(volume.path(), &current, &next).is_err());
    }

    #[test]
    fn all_library_settings_round_trip_and_display_does_not_rewrite_databases() {
        let volume = tempfile::tempdir().unwrap();
        rbl_export::create_library(volume.path(), None, &[], None).unwrap();
        let current = read(volume.path()).unwrap();
        let mut next = current.clone();
        let library = next.library.as_mut().unwrap();
        library.device_name = "FRIDAY".into();
        library.background_color_type = 8;
        library.categories[0].visible = !library.categories[0].visible;
        library.sorts[0].visible = !library.sorts[0].visible;
        library.sub_column = Some(library.sorts[1].menu_item);
        library.colors[0].name = "Favorites".into();
        next.device_library_background = Some(4);
        write_changes(volume.path(), &current, &next).unwrap();
        let saved = read(volume.path()).unwrap();
        let library = saved.library.as_ref().unwrap();
        assert_eq!(library.device_name, "FRIDAY");
        assert_eq!(library.background_color_type, 8);
        assert_eq!(
            library.sub_column,
            next.library.as_ref().unwrap().sub_column
        );
        assert_eq!(
            library.categories,
            rbl_onelibrary::settings::renumbered(&next.library.as_ref().unwrap().categories)
        );
        assert_eq!(
            library.sorts,
            rbl_onelibrary::settings::renumbered(&next.library.as_ref().unwrap().sorts)
        );
        assert_eq!(library.colors[0].name, "Favorites");
        assert_eq!(saved.device_library_background, Some(4));
        let root = export_root(volume.path());
        let pdb = std::fs::read(root.join("rekordbox/export.pdb")).unwrap();
        let property = rbl_pdb::Pdb::parse(&pdb).unwrap().property().unwrap();
        assert_eq!(property.device_name, "FRIDAY");
        let db = std::fs::read(root.join("rekordbox/exportLibrary.db")).unwrap();
        let mut display = saved.clone();
        display.dev = Some(DevSetting {
            color: WaveformColor::TriBand,
            ..Default::default()
        });
        write_changes(volume.path(), &saved, &display).unwrap();
        assert_eq!(
            std::fs::read(root.join("rekordbox/exportLibrary.db")).unwrap(),
            db
        );
        assert_eq!(
            std::fs::read(root.join("rekordbox/export.pdb")).unwrap(),
            pdb
        );
    }

    #[test]
    fn hidden_root_corruption_and_pending_recovery() {
        let volume = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(volume.path().join(".PIONEER")).unwrap();
        let current = read(volume.path()).unwrap();
        let mut next = current.clone();
        next.dev = Some(DevSetting::default());
        write_changes(volume.path(), &current, &next).unwrap();
        let path = volume.path().join(".PIONEER/DEVSETTING.DAT");
        assert!(path.exists());
        std::fs::write(&path, b"damaged").unwrap();
        assert!(read(volume.path()).is_err());
        assert!(write(volume.path(), &next).is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"damaged");
        std::fs::create_dir(volume.path().join(".exportstudio-publication")).unwrap();
        assert!(read(volume.path())
            .unwrap_err()
            .to_string()
            .contains("recovery"));
        assert!(write(volume.path(), &next).is_err());
    }

    #[test]
    fn missing_mount_is_not_created() {
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("unplugged");
        assert!(read(&missing).is_err());
        let current = read(parent.path()).unwrap();
        assert!(write(&missing, &current).is_err());
        assert!(!missing.exists());
    }

    #[cfg(unix)]
    #[test]
    fn linked_export_root_is_not_written() {
        let volume = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), volume.path().join("PIONEER")).unwrap();
        assert!(read(volume.path()).is_err());
        let current = read(elsewhere.path()).unwrap();
        assert!(write(
            volume.path(),
            &DeviceSettings {
                dev: Some(DevSetting::default()),
                ..current
            }
        )
        .is_err());
        assert!(!elsewhere.path().join("DEVSETTING.DAT").exists());
    }
}
