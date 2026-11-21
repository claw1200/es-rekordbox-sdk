//! macOS mounts every volume but the boot volume under `/Volumes`; `diskutil` describes, ejects
//! and erases them.

use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use super::Mounted;
use crate::{fail, Error, Result};

const VOLUMES: &str = "/Volumes";
const DESCRIBE_LIMIT: Duration = Duration::from_secs(3);
const EJECT_LIMIT: Duration = Duration::from_secs(30);
const ERASE_LIMIT: Duration = Duration::from_secs(120);

/// Run a command, killing it when it takes longer than `limit`.
fn run(program: &str, args: &[&str], limit: Duration) -> std::io::Result<Option<Output>> {
    let mut child = Command::new(program).args(args).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let started = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return child.wait_with_output().map(Some);
        }
        if started.elapsed() > limit {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn disk_info(path: &Path) -> plist::Dictionary {
    let Ok(Some(out)) = run("diskutil", &["info", "-plist", &path.to_string_lossy()], DESCRIBE_LIMIT) else {
        return plist::Dictionary::new();
    };
    plist::Value::from_reader(std::io::Cursor::new(out.stdout)).ok().and_then(plist::Value::into_dictionary).unwrap_or_default()
}

fn flag(info: &plist::Dictionary, key: &str) -> bool {
    info.get(key).and_then(plist::Value::as_boolean).unwrap_or(false)
}

fn text(info: &plist::Dictionary, key: &str) -> String {
    info.get(key).and_then(plist::Value::as_string).unwrap_or_default().to_owned()
}

fn ejectable(info: &plist::Dictionary) -> bool {
    (flag(info, "RemovableMedia") || flag(info, "Ejectable")) && !flag(info, "Internal")
}

fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|m| m.dev())
}

pub(super) fn mounted() -> Vec<Mounted> {
    let Ok(entries) = std::fs::read_dir(VOLUMES) else { return Vec::new() };
    let boot = device_of(Path::new("/"));
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && device_of(path) != boot)
        .map(|path| {
            let info = disk_info(&path);
            Mounted {
                name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                file_system: Some(text(&info, "FilesystemName"))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| text(&info, "FilesystemType")),
                ejectable: ejectable(&info),
                path,
            }
        })
        .collect()
}

fn detail(output: &Output, fallback: &str) -> Error {
    let text = String::from_utf8_lossy(if output.stderr.is_empty() { &output.stdout } else { &output.stderr }).trim().to_owned();
    Error::Message(if text.is_empty() { fallback.to_owned() } else { text })
}

pub(super) fn eject(volume: &Path) -> Result<()> {
    match run("diskutil", &["eject", &volume.canonicalize()?.to_string_lossy()], EJECT_LIMIT)? {
        None => fail("Ejecting the device took too long"),
        Some(out) if out.status.success() => Ok(()),
        Some(out) => Err(detail(&out, "Couldn’t eject the device")),
    }
}

pub(super) fn erase_as_fat32(volume: &Path, label: &str) -> Result<()> {
    let info = disk_info(volume);
    if !ejectable(&info) {
        return fail("That device can’t be formatted");
    }
    let device = text(&info, "DeviceIdentifier");
    if device.is_empty() {
        return fail("Couldn’t identify the device");
    }
    match run("diskutil", &["eraseVolume", "FAT32", label, &device], ERASE_LIMIT)? {
        None => fail("Formatting the device took too long"),
        Some(out) if out.status.success() => Ok(()),
        Some(out) => Err(detail(&out, "Couldn’t format the device")),
    }
}
