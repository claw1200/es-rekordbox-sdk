//! Choosing what Windows should remove to eject a drive, kept free of Win32 calls so it is tested
//! on every platform. Ported from `rbl-devices`.

use std::path::Path;

/// The drive letter of a Windows drive root such as `E:\`. A volume mounted in a folder has none.
pub(super) fn drive_letter(path: &Path) -> Option<char> {
    let text = path.as_os_str().to_string_lossy();
    let mut chars = text.chars();
    match (chars.next(), chars.next(), chars.next(), chars.next()) {
        (Some(letter), Some(':'), Some('\\' | '/'), None) if letter.is_ascii_alphabetic() => {
            Some(letter.to_ascii_uppercase())
        }
        _ => None,
    }
}

/// The strings in a `REG_MULTI_SZ` buffer: each ends in a NUL and an empty string ends the list.
pub(super) fn multi_strings(buffer: &[u16]) -> impl Iterator<Item = &[u16]> {
    buffer.split(|&unit| unit == 0).take_while(|item| !item.is_empty())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum EjectTarget<N> {
    /// Ask Plug and Play to remove this device node, as "Safely Remove Hardware" does.
    Device(N),
    /// No device is removable but the media is (a card in a built-in SD reader): lock, dismount
    /// and eject the media, as Explorer's Eject does.
    Media,
    /// Neither, such as a disk inside the computer.
    Refuse,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct DeviceNode {
    /// `CM_DEVCAP_REMOVABLE`: removable from its immediate parent, which is what puts a device in
    /// "Safely Remove Hardware".
    pub(super) removable: bool,
    /// A USB interface of a composite device (`USB\VID_…&PID_…&MI_nn\…`).
    pub(super) interface: bool,
    /// Ejecting a hub takes down everything plugged into it (a dock's keyboard, network, audio).
    pub(super) hub: bool,
}

pub(super) fn is_interface_id(instance_id: &str) -> bool {
    let mut parts = instance_id.split('\\');
    parts.next().is_some_and(|bus| bus.eq_ignore_ascii_case("USB"))
        && parts.next().is_some_and(|id| id.to_ascii_uppercase().contains("&MI_"))
}

/// A root hub (`USB\ROOT_HUB…`), a device with the hub class among its compatible ids
/// (`USB\Class_09…`), or one run by a hub driver (`usbhub`, `USBHUB3`).
pub(super) fn is_hub<'a>(instance_id: &str, mut compatible_ids: impl Iterator<Item = &'a str>, service: &str) -> bool {
    let starts = |text: &str, prefix: &str| text.get(..prefix.len()).is_some_and(|head| head.eq_ignore_ascii_case(prefix));
    starts(instance_id, "USB\\ROOT_HUB")
        || compatible_ids.any(|id| starts(id, "USB\\Class_09"))
        || service.eq_ignore_ascii_case("usbhub")
        || service.eq_ignore_ascii_case("usbhub3")
}

/// For a stick the removable device is the disk's parent. On a composite device (many card
/// readers) that parent is an interface node and the removable device is one level higher, so
/// the walk climbs a second level only through an interface node. Anything further reaches hubs
/// and controllers: a dock's external hub and a Thunderbolt USB controller can both report
/// removable, and ejecting either would take down far more than the stick.
pub(super) fn eject_target<N: Copy>(
    disk: N,
    removable_media: bool,
    mut parent: impl FnMut(N) -> Option<N>,
    mut describe: impl FnMut(N) -> DeviceNode,
) -> EjectTarget<N> {
    let ejectable = |node: DeviceNode| node.removable && !node.hub;
    if let Some(up) = parent(disk) {
        let first = describe(up);
        if ejectable(first) {
            return EjectTarget::Device(up);
        }
        if first.interface && !first.hub {
            if let Some(device) = parent(up) {
                if ejectable(describe(device)) {
                    return EjectTarget::Device(device);
                }
            }
        }
    }
    if ejectable(describe(disk)) {
        EjectTarget::Device(disk)
    } else if removable_media {
        EjectTarget::Media
    } else {
        EjectTarget::Refuse
    }
}

/// Why Windows refused to let the stick go, from its `PNP_VETO_TYPE` (cfg.h) and the process,
/// service or device it names.
pub(super) fn veto_message(veto_type: i32, name: &str) -> String {
    const PENDING_CLOSE: i32 = 2;
    const WINDOWS_APP: i32 = 3;
    const WINDOWS_SERVICE: i32 = 4;
    const OUTSTANDING_OPEN: i32 = 5;
    const INSUFFICIENT_RIGHTS: i32 = 12;
    const ALREADY_REMOVED: i32 = 13;
    // A program arrives as an NT path such as `\Device\HarddiskVolume3\Windows\explorer.exe`.
    let program = name.trim().rsplit(['\\', '/']).next().unwrap_or_default().trim();
    match veto_type {
        WINDOWS_APP | WINDOWS_SERVICE if !program.is_empty() => {
            format!("The USB is in use by {program}. Close it, then eject the USB again.")
        }
        WINDOWS_APP | WINDOWS_SERVICE | OUTSTANDING_OPEN | PENDING_CLOSE => {
            "The USB is in use by another program. Close any window or program using it, then eject the USB again."
                .to_owned()
        }
        INSUFFICIENT_RIGHTS => "Windows does not allow this account to eject the USB.".to_owned(),
        ALREADY_REMOVED => "The USB is no longer connected.".to_owned(),
        _ => "Windows could not eject the USB. It may be in use.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::EjectTarget::{Device, Media, Refuse};
    use super::*;

    #[test]
    fn only_a_drive_root_has_a_letter() {
        assert_eq!(drive_letter(Path::new("E:\\")), Some('E'));
        assert_eq!(drive_letter(Path::new("f:/")), Some('F'));
        assert_eq!(drive_letter(Path::new("E:")), None);
        assert_eq!(drive_letter(Path::new("E:\\Music")), None);
        assert_eq!(drive_letter(Path::new("\\\\.\\E:")), None);
        assert_eq!(drive_letter(Path::new("/Volumes/USB")), None);
        assert_eq!(drive_letter(Path::new("1:\\")), None);
    }

    #[test]
    fn a_multi_string_lists_each_entry_once() {
        let wide = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
        let mut buffer = wide("\\\\?\\usbstor#disk#a");
        buffer.push(0);
        buffer.extend(wide("\\\\?\\scsi#disk#b"));
        buffer.extend([0, 0, 0]);
        let items: Vec<String> = multi_strings(&buffer).map(String::from_utf16_lossy).collect();
        assert_eq!(items, ["\\\\?\\usbstor#disk#a", "\\\\?\\scsi#disk#b"]);
        assert_eq!(multi_strings(&[0, 0]).count(), 0);
        assert_eq!(multi_strings(&[]).count(), 0);
    }

    /// A device tree as (node, parent, kind) rows.
    fn target(tree: &[(u32, Option<u32>, DeviceNode)], disk: u32, removable_media: bool) -> EjectTarget<u32> {
        let row = |node: u32| tree.iter().find(|row| row.0 == node).copied();
        eject_target(
            disk,
            removable_media,
            |node| row(node).and_then(|row| row.1),
            |node| row(node).map(|row| row.2).unwrap_or_default(),
        )
    }

    const FIXED: DeviceNode = DeviceNode { removable: false, interface: false, hub: false };
    const REMOVABLE: DeviceNode = DeviceNode { removable: true, interface: false, hub: false };
    const INTERFACE: DeviceNode = DeviceNode { removable: false, interface: true, hub: false };
    const HUB: DeviceNode = DeviceNode { removable: false, interface: false, hub: true };
    const REMOVABLE_HUB: DeviceNode = DeviceNode { removable: true, interface: false, hub: true };

    #[test]
    fn a_stick_ejects_its_usb_device() {
        let tree = [(1, Some(2), FIXED), (2, Some(3), REMOVABLE), (3, Some(4), HUB), (4, None, REMOVABLE)];
        assert_eq!(target(&tree, 1, false), Device(2));
        assert_eq!(target(&tree, 1, true), Device(2));
    }

    #[test]
    fn a_composite_reader_ejects_the_device_above_its_interface() {
        let tree = [(1, Some(2), FIXED), (2, Some(3), INTERFACE), (3, Some(4), REMOVABLE), (4, None, HUB)];
        assert_eq!(target(&tree, 1, false), Device(3));
        assert_eq!(target(&tree, 1, true), Device(3));
    }

    #[test]
    fn a_reader_in_a_dock_never_ejects_the_dock_hub() {
        let tree = [(1, Some(2), FIXED), (2, Some(3), FIXED), (3, Some(4), REMOVABLE_HUB), (4, None, HUB)];
        assert_eq!(target(&tree, 1, true), Media);
        assert_eq!(target(&tree, 1, false), Refuse);
        let unflagged_hub = [(1, Some(2), FIXED), (2, Some(3), FIXED), (3, Some(4), REMOVABLE), (4, None, HUB)];
        assert_eq!(target(&unflagged_hub, 1, true), Media);
        assert_eq!(target(&unflagged_hub, 1, false), Refuse);
        let above_interface = [(1, Some(2), FIXED), (2, Some(3), INTERFACE), (3, None, REMOVABLE_HUB)];
        assert_eq!(target(&above_interface, 1, true), Media);
        let above_disk = [(1, Some(2), FIXED), (2, None, REMOVABLE_HUB)];
        assert_eq!(target(&above_disk, 1, false), Refuse);
    }

    #[test]
    fn the_walk_never_reaches_a_removable_controller() {
        let thunderbolt = [(1, Some(2), FIXED), (2, Some(3), FIXED), (3, Some(4), HUB), (4, None, REMOVABLE)];
        assert_eq!(target(&thunderbolt, 1, false), Refuse);
        assert_eq!(target(&thunderbolt, 1, true), Media);
        let through_interface = [(1, Some(2), FIXED), (2, Some(3), INTERFACE), (3, Some(4), FIXED), (4, None, REMOVABLE)];
        assert_eq!(target(&through_interface, 1, true), Media);
        let removable_disk = [(1, Some(2), REMOVABLE), (2, Some(3), FIXED), (3, None, FIXED)];
        assert_eq!(target(&removable_disk, 1, false), Device(1));
    }

    #[test]
    fn an_internal_disk_is_refused_and_a_built_in_card_reader_ejects_the_media() {
        let internal = [(1, Some(2), FIXED), (2, Some(3), FIXED), (3, None, FIXED)];
        assert_eq!(target(&internal, 1, false), Refuse);
        assert_eq!(target(&internal, 1, true), Media);
        assert_eq!(target(&[(1, None, FIXED)], 1, false), Refuse);
    }

    #[test]
    fn interface_and_hub_nodes_are_told_apart_by_their_ids() {
        assert!(is_interface_id("USB\\VID_05AC&PID_8600&MI_00\\6&C3BB1E1&0&0000"));
        assert!(is_interface_id("usb\\vid_05ac&pid_8600&mi_03\\6&c3bb1e1&0&0003"));
        assert!(!is_interface_id("USB\\VID_05AC&PID_8600\\5&1F3E3F24&0&3"));
        assert!(!is_interface_id("USB\\VID_0781&PID_55BB\\MSFT30323233363654343031313833"));
        assert!(!is_interface_id("USBSTOR\\DISK&VEN_X&MI_00\\1"));
        assert!(!is_interface_id("SCSI\\DISK&VEN_SANDISK\\7&1"));
        assert!(!is_interface_id(""));

        let none = std::iter::empty::<&str>;
        assert!(is_hub("USB\\ROOT_HUB30\\7&27C23B87&0&0", none(), "USBHUB3"));
        assert!(is_hub("USB\\ROOT_HUB\\4&1", none(), ""));
        assert!(is_hub(
            "USB\\VID_05E3&PID_0610\\6&1",
            ["USB\\USB20_HUB", "USB\\Class_09&SubClass_00", "USB\\Class_09"].into_iter(),
            "",
        ));
        assert!(is_hub("USB\\VID_2109&PID_0817\\6&2", none(), "usbhub"));
        assert!(!is_hub(
            "USB\\VID_0781&PID_55BB\\MSFT3032",
            ["USB\\Class_08&SubClass_06&Prot_62", "USB\\Class_08"].into_iter(),
            "UASPStor",
        ));
        assert!(!is_hub("USB\\VID_05AC&PID_8600\\5&1", ["USB\\Class_0E"].into_iter(), "usbccgp"));
    }

    #[test]
    fn a_veto_says_what_holds_the_usb_without_a_raw_os_error() {
        let app = veto_message(3, "\\Device\\HarddiskVolume7\\Windows\\explorer.exe");
        assert!(app.contains("explorer.exe"), "{app}");
        assert!(veto_message(5, "USBSTOR\\Disk").contains("another program"));
        assert!(veto_message(3, "").contains("another program"));
        assert!(veto_message(13, "").contains("no longer connected"));
        for veto in 0..16 {
            assert!(!veto_message(veto, "x").contains("os error"));
        }
    }
}
