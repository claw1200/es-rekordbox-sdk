//! Windows gives each volume a drive letter. A drive can be ejected when Plug and Play offers its
//! device for safe removal, or its media is removable; ejecting goes through Plug and Play, as
//! "Safely Remove Hardware" does. Ported from `rbl-devices`.
//!
//! Volumes and disks are only queried through handles with no read or write access, which need
//! no administrator rights. An eject request makes Windows ask every program holding a file on
//! the stick to let go, flush and dismount, and refuse (a veto) while anything is still open, so
//! a busy stick is never forced. Locking the volume instead (`FSCTL_LOCK_VOLUME`) fails with
//! "Access is denied" while any program, even Explorer or the search indexer, has a file open on
//! it, so the lock is kept for media with no removable device above it, such as a card in a
//! built-in SD reader, where it is what Explorer's Eject does.
#![allow(unsafe_code, reason = "Plug and Play and volume queries are Win32 FFI; every handle is owned and closed")]

use std::{
    ffi::OsStr,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    time::Duration,
};

use windows_sys::Win32::{
    Devices::{
        DeviceAndDriverInstallation::{
            CM_Get_DevNode_Registry_PropertyW, CM_Get_Device_IDW, CM_Get_Device_Interface_ListW,
            CM_Get_Device_Interface_List_SizeW, CM_Get_Device_Interface_PropertyW, CM_Get_Parent, CM_Locate_DevNodeW,
            CM_Request_Device_EjectW, PNP_VetoInsufficientRights, PNP_VetoOutstandingOpen, PNP_VetoTypeUnknown,
            CM_DEVCAP_REMOVABLE, CM_DRP_CAPABILITIES, CM_DRP_COMPATIBLEIDS, CM_DRP_SERVICE,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_LOCATE_DEVNODE_NORMAL, CONFIGRET, CR_BUFFER_SMALL, CR_SUCCESS,
            PNP_VETO_TYPE,
        },
        Properties::{DEVPKEY_Device_InstanceId, DEVPROPTYPE},
    },
    Foundation::{ERROR_ACCESS_DENIED, GENERIC_READ, GENERIC_WRITE, INVALID_HANDLE_VALUE, MAX_PATH},
    Storage::FileSystem::{
        CreateFileW, GetDriveTypeW, FILE_DEVICE_DISK, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    },
    System::{
        Ioctl::{
            FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, GUID_DEVINTERFACE_DISK, IOCTL_STORAGE_EJECT_MEDIA,
            IOCTL_STORAGE_GET_DEVICE_NUMBER, STORAGE_DEVICE_NUMBER,
        },
        WindowsProgramming::DRIVE_REMOVABLE,
        IO::DeviceIoControl,
    },
};

use super::safe_removal::{
    drive_letter, eject_target, is_hub, is_interface_id, multi_strings, veto_message, DeviceNode, EjectTarget,
};
use super::Mounted;
use crate::{fail, Error, Result};

/// Explorer and other programs can take a moment to close their handles after the first
/// query-remove, so a vetoed request is repeated.
const ATTEMPTS: usize = 3;
const RETRY_DELAY: Duration = Duration::from_millis(500);

/// A device instance id is at most 200 characters (`MAX_DEVICE_ID_LEN`).
const DEVICE_ID_UNITS: usize = 256;

pub(super) fn mounted() -> Vec<Mounted> {
    let system = system_drive_letter();
    sysinfo::Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter_map(|disk| {
            let letter = drive_letter(disk.mount_point()).filter(|letter| Some(*letter) != system)?;
            let ejectable = can_eject(letter);
            Some(Mounted {
                path: disk.mount_point().to_owned(),
                name: display_name(disk.name(), letter, ejectable),
                file_system: disk.file_system().to_string_lossy().into_owned(),
                ejectable,
            })
        })
        .collect()
}

fn system_drive_letter() -> Option<char> {
    std::env::var("SystemDrive").ok()?.chars().next().map(|letter| letter.to_ascii_uppercase())
}

/// The volume label, or the name Explorer shows for an unlabelled drive.
fn display_name(label: &OsStr, letter: char, ejectable: bool) -> String {
    let label = label.to_string_lossy();
    if !label.trim().is_empty() {
        label.into_owned()
    } else if ejectable {
        format!("USB Drive ({letter}:)")
    } else {
        format!("Local Disk ({letter}:)")
    }
}

/// A failed Plug and Play query falls back to the drive type, so a stick Windows calls removable
/// is still offered.
fn can_eject(letter: char) -> bool {
    removal_for(letter).map_or_else(|_| has_removable_media(letter), |target| target != EjectTarget::Refuse)
}

pub(super) fn eject(volume: &Path) -> Result<()> {
    let Some(letter) = drive_letter(volume) else {
        return fail("Only a drive with a letter can be ejected");
    };
    eject_drive(letter).map_err(|error| Error::Message(error.to_string()))
}

pub(super) fn erase_as_fat32(_volume: &Path, _label: &str) -> Result<()> {
    fail("Formatting a USB isn’t supported on Windows yet")
}

fn eject_drive(letter: char) -> io::Result<()> {
    match removal_for(letter)? {
        EjectTarget::Device(node) => request_eject(node),
        EjectTarget::Media => eject_media(&volume_device_path(letter)),
        EjectTarget::Refuse => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Windows does not offer this drive for safe removal, so it cannot be ejected here.",
        )),
    }
}

fn removal_for(letter: char) -> io::Result<EjectTarget<u32>> {
    let volume = device_number(&volume_device_path(letter))?;
    let disk = disk_node(volume.DeviceNumber)?;
    Ok(eject_target(disk, has_removable_media(letter), parent, describe))
}

fn has_removable_media(letter: char) -> bool {
    // SAFETY: NUL-terminated drive root such as `E:\`.
    unsafe { GetDriveTypeW(wide(&format!("{letter}:\\")).as_ptr()) == DRIVE_REMOVABLE }
}

fn volume_device_path(letter: char) -> Vec<u16> {
    wide(&format!("\\\\.\\{letter}:"))
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

fn until_nul(buffer: &[u16]) -> &[u16] {
    &buffer[..buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len())]
}

/// The disk number behind a volume or disk interface path. The handle is dropped before this
/// returns, so it never keeps the stick busy.
fn device_number(path: &[u16]) -> io::Result<STORAGE_DEVICE_NUMBER> {
    // SAFETY: NUL-terminated name and null optional pointers; synchronous handle.
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: CreateFileW returned a new, valid handle owned by this function.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut number = STORAGE_DEVICE_NUMBER { DeviceType: 0, DeviceNumber: 0, PartitionNumber: 0 };
    let mut returned = 0;
    // SAFETY: Live handle; the output buffer is a STORAGE_DEVICE_NUMBER of the size given.
    let ok = unsafe {
        DeviceIoControl(
            handle.as_raw_handle(),
            IOCTL_STORAGE_GET_DEVICE_NUMBER,
            std::ptr::null(),
            0,
            (&raw mut number).cast(),
            u32::try_from(size_of::<STORAGE_DEVICE_NUMBER>()).unwrap_or(u32::MAX),
            &raw mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(number)
}

fn pnp_error(what: &str, code: CONFIGRET) -> io::Error {
    io::Error::other(format!("Windows could not find the USB device ({what}, CONFIGRET {code})."))
}

/// The device node of disk number `number`, found by asking each present disk interface for its
/// number.
fn disk_node(number: u32) -> io::Result<u32> {
    let interfaces = disk_interfaces()?;
    for interface in multi_strings(&interfaces) {
        let path: Vec<u16> = interface.iter().copied().chain(Some(0)).collect();
        let Ok(found) = device_number(&path) else { continue };
        if found.DeviceType != FILE_DEVICE_DISK || found.DeviceNumber != number {
            continue;
        }
        let id = instance_id(&path)?;
        let mut node = 0;
        // SAFETY: NUL-terminated instance id and a valid out pointer.
        let code = unsafe { CM_Locate_DevNodeW(&raw mut node, id.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
        if code != CR_SUCCESS {
            return Err(pnp_error("locate disk", code));
        }
        return Ok(node);
    }
    Err(io::Error::new(io::ErrorKind::NotFound, "The USB is no longer connected."))
}

/// Every present disk interface path, as a multi-string. The list can grow between asking for its
/// size and reading it, so a short buffer is retried.
fn disk_interfaces() -> io::Result<Vec<u16>> {
    for _ in 0..ATTEMPTS {
        let mut length = 0;
        // SAFETY: Valid out pointer and interface GUID; no device id filter.
        let code = unsafe {
            CM_Get_Device_Interface_List_SizeW(
                &raw mut length,
                &GUID_DEVINTERFACE_DISK,
                std::ptr::null(),
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        if code != CR_SUCCESS {
            return Err(pnp_error("list disks", code));
        }
        let mut buffer = vec![0u16; length as usize];
        // SAFETY: The buffer holds exactly `length` UTF-16 units.
        let code = unsafe {
            CM_Get_Device_Interface_ListW(
                &GUID_DEVINTERFACE_DISK,
                std::ptr::null(),
                buffer.as_mut_ptr(),
                length,
                CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
            )
        };
        match code {
            CR_SUCCESS => return Ok(buffer),
            CR_BUFFER_SMALL => {}
            _ => return Err(pnp_error("list disks", code)),
        }
    }
    Err(pnp_error("list disks", CR_BUFFER_SMALL))
}

/// The device instance id of a disk interface, NUL-terminated.
fn instance_id(interface: &[u16]) -> io::Result<Vec<u16>> {
    let mut buffer = vec![0u16; DEVICE_ID_UNITS];
    let mut kind: DEVPROPTYPE = 0;
    let mut size = u32::try_from(buffer.len() * 2).unwrap_or(u32::MAX);
    // SAFETY: NUL-terminated interface path; the buffer holds `size` bytes.
    let code = unsafe {
        CM_Get_Device_Interface_PropertyW(
            interface.as_ptr(),
            &DEVPKEY_Device_InstanceId,
            &raw mut kind,
            buffer.as_mut_ptr().cast(),
            &raw mut size,
            0,
        )
    };
    if code != CR_SUCCESS {
        return Err(pnp_error("disk id", code));
    }
    Ok(until_nul(&buffer).iter().copied().chain(Some(0)).collect())
}

/// A property Windows does not return reads as absent (not removable, not an interface, not a
/// hub), so a failed query never widens what can be ejected.
fn describe(node: u32) -> DeviceNode {
    let id = device_id(node).unwrap_or_default();
    let compatible = registry_property(node, CM_DRP_COMPATIBLEIDS).unwrap_or_default();
    let service = registry_property(node, CM_DRP_SERVICE).unwrap_or_default();
    let service = String::from_utf16_lossy(multi_strings(&service).next().unwrap_or_default());
    let compatible: Vec<String> = multi_strings(&compatible).map(String::from_utf16_lossy).collect();
    DeviceNode {
        removable: is_removable(node),
        interface: is_interface_id(&id),
        hub: is_hub(&id, compatible.iter().map(String::as_str), &service),
    }
}

/// The device instance id of `node`, such as `USB\VID_0781&PID_55BB\…`.
fn device_id(node: u32) -> Option<String> {
    let mut buffer = [0u16; DEVICE_ID_UNITS];
    let units_before_nul = u32::try_from(DEVICE_ID_UNITS - 1).ok()?;
    // SAFETY: Valid device node; one unit is left spare so the id stays NUL-terminated.
    let code = unsafe { CM_Get_Device_IDW(node, buffer.as_mut_ptr(), units_before_nul, 0) };
    (code == CR_SUCCESS).then(|| String::from_utf16_lossy(until_nul(&buffer)))
}

/// A string or multi-string registry property of `node`, with two spare NULs at the end so it
/// always parses as a terminated multi-string.
fn registry_property(node: u32, property: u32) -> Option<Vec<u16>> {
    let mut size = 0u32;
    // SAFETY: Size query: null buffer with a zero size; the type out pointer is optional.
    let code = unsafe {
        CM_Get_DevNode_Registry_PropertyW(node, property, std::ptr::null_mut(), std::ptr::null_mut(), &raw mut size, 0)
    };
    if code != CR_BUFFER_SMALL || size == 0 {
        return None;
    }
    let units = (size as usize).div_ceil(2);
    let mut buffer = vec![0u16; units + 2];
    let mut size = u32::try_from(units * 2).ok()?;
    // SAFETY: The buffer holds at least `size` bytes; the type out pointer is optional.
    let code = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            node,
            property,
            std::ptr::null_mut(),
            buffer.as_mut_ptr().cast(),
            &raw mut size,
            0,
        )
    };
    (code == CR_SUCCESS).then_some(buffer)
}

fn is_removable(node: u32) -> bool {
    let mut capabilities = 0u32;
    let mut size = u32::try_from(size_of::<u32>()).unwrap_or(4);
    // SAFETY: The buffer is one u32 of the size given; the type out pointer is optional.
    let code = unsafe {
        CM_Get_DevNode_Registry_PropertyW(
            node,
            CM_DRP_CAPABILITIES,
            std::ptr::null_mut(),
            (&raw mut capabilities).cast(),
            &raw mut size,
            0,
        )
    };
    code == CR_SUCCESS && capabilities & CM_DEVCAP_REMOVABLE != 0
}

fn parent(node: u32) -> Option<u32> {
    let mut up = 0;
    // SAFETY: Valid out pointer and a device node from Plug and Play.
    (unsafe { CM_Get_Parent(&raw mut up, node, 0) } == CR_SUCCESS).then_some(up)
}

fn is_access_denied(error: &io::Error) -> bool {
    error.raw_os_error().and_then(|code| u32::try_from(code).ok()) == Some(ERROR_ACCESS_DENIED)
}

/// The lock is refused while any program has a file open on the volume, which is reported as "in
/// use" after a short retry and leaves the card mounted.
fn eject_media(volume: &[u16]) -> io::Result<()> {
    // SAFETY: NUL-terminated name and null optional pointers; synchronous handle.
    let raw = unsafe {
        CreateFileW(
            volume.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        let error = io::Error::last_os_error();
        // The share mode lets open files through, so a refusal here is down to rights or a
        // removable-storage policy.
        if is_access_denied(&error) {
            return Err(io::Error::other(veto_message(PNP_VetoInsufficientRights, "")));
        }
        return Err(error);
    }
    // SAFETY: CreateFileW returned a new, valid handle owned by this function.
    let handle = unsafe { OwnedHandle::from_raw_handle(raw) };
    let control = |code: u32| -> io::Result<()> {
        let mut returned = 0;
        // SAFETY: Live volume handle; these control codes use no input or output buffer.
        let ok = unsafe {
            DeviceIoControl(
                handle.as_raw_handle(),
                code,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                0,
                &raw mut returned,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    };
    let denied = |result: &io::Result<()>| matches!(result, Err(error) if is_access_denied(error));
    let mut locked = control(FSCTL_LOCK_VOLUME);
    for _ in 1..ATTEMPTS {
        if !denied(&locked) {
            break;
        }
        std::thread::sleep(RETRY_DELAY);
        locked = control(FSCTL_LOCK_VOLUME);
    }
    if denied(&locked) {
        return Err(io::Error::other(veto_message(PNP_VetoOutstandingOpen, "")));
    }
    locked?;
    // Dropping the handle afterwards unlocks the now dismounted volume.
    control(FSCTL_DISMOUNT_VOLUME)?;
    control(IOCTL_STORAGE_EJECT_MEDIA)
}

fn request_eject(node: u32) -> io::Result<()> {
    let mut last = String::new();
    for attempt in 0..ATTEMPTS {
        if attempt > 0 {
            std::thread::sleep(RETRY_DELAY);
        }
        let mut veto: PNP_VETO_TYPE = PNP_VetoTypeUnknown;
        let mut name = [0u16; MAX_PATH as usize];
        // SAFETY: Valid device node; the veto name buffer holds MAX_PATH units. Passing a name
        // buffer also stops Windows showing its own balloon.
        let code = unsafe { CM_Request_Device_EjectW(node, &raw mut veto, name.as_mut_ptr(), MAX_PATH, 0) };
        if code == CR_SUCCESS && veto == PNP_VetoTypeUnknown {
            return Ok(());
        }
        last = veto_message(veto, &String::from_utf16_lossy(until_nul(&name)));
    }
    Err(io::Error::other(last))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_system_drive_is_never_offered() {
        let system = system_drive_letter();
        assert!(system.is_some());
        assert!(mounted().iter().all(|volume| drive_letter(&volume.path) != system));
    }

    #[test]
    fn an_unlabelled_drive_is_named_as_explorer_names_it() {
        assert_eq!(display_name(OsStr::new("DJ STICK"), 'E', true), "DJ STICK");
        assert_eq!(display_name(OsStr::new(""), 'E', true), "USB Drive (E:)");
        assert_eq!(display_name(OsStr::new("  "), 'D', false), "Local Disk (D:)");
    }
}
