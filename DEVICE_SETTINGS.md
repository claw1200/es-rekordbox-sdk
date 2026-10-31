# Device Settings And Retained Device APIs

Groundwork for [issue #5](https://github.com/claw1200/es-rekordbox-sdk/issues/5).
This compares the device crates after the settings port. It does not decide which
remaining capabilities to port or remove, and does not complete the other three
crate comparisons requested by that issue.

## Settings Port

The public [rb-device settings API](crates/rb-device/src/settings.rs) ports the
supported behavior from [rbl-devices settings](crates/rbl-devices/src/settings.rs):

- `DEVSETTING.DAT`: waveform color (Blue/RGB/3Band), overview (Half/Full), key
  display (Classic/Alphanumeric), current position (Center/Left), CRC validation.
- Existing `exportLibrary.db`: name, category and sort visibility/order,
  sub-column, color labels and OneLibrary background, through the shipped
  [OneLibrary settings codec](crates/rbl-onelibrary/src/settings.rs).
- Existing `export.pdb`: Device Library background; mirrored name/color labels
  when changing the OneLibrary settings. Neither API provides independent
  name/color-label editing for a Device Library-only stick.
- Visible/hidden export roots and changed-group-only durable publication.

The port deliberately differs in safety behavior: reads return errors for
unreadable/unsupported files rather than treating them as absent, and never
recover implicitly. The snapshot writer rejects stale settings under the
exclusive lease. Missing mounts and symbolic-link settings paths are refused.
Display equality compares the encoded state, so saved edits remain reusable
snapshots. Initially lock-free reads retry under a lease if the first writer
appears, and the settings publisher refuses pending recovery after obtaining its
exclusive lease. Strict OneLibrary and PDB codec paths reject decoding failures,
invalid page chains and unsupported property rows instead of substituting defaults.
All unknown display bytes (including header/padding) survive a supported edit;
unknown values of the four editable enums are refused rather than normalized.
Settings use the shipped export journal, so explicit
[rb-usb recovery](crates/rb-usb/src/lib.rs) handles interrupted saves.

Waveform divisions remain unknown/unimplemented on both sides. These APIs do not
edit MYSETTING/MYSETTING2/DJMMYSETTING player/mixer preferences. Settings file I/O
is platform-independent; platform-specific volume enumeration is separate.

Tests cover a captured rekordbox 7.2.8 display file, generated encrypted libraries,
byte preservation, damaged files, selective writes, stale snapshots, hidden roots,
missing/linked destinations, and settings surviving an export rebuild in the
[SDK export test](crates/rb-usb/src/lib.rs). Physical CDJ behavior is not validated
by those tests.

## Remaining Differences

| Surface | Retained `rbl-devices` | Shipped SDK / Export Studio counterpart |
| --- | --- | --- |
| Volume discovery and identity | [list/volume_id](crates/rbl-devices/src/lib.rs): sysinfo-based enumeration, macOS/Linux/Windows filtering, removable and external mounts, Unix device-number identity, fake-volume environment hook. | [rb-device volumes](crates/rb-device/src/volumes.rs): `/Volumes` and `diskutil`, mount-path identity, boot-volume filtering, library/ejectability checks; an explicit test-root argument. Not cross-platform discovery parity. |
| Mount watching/remounts | [MountWatcher](crates/rbl-devices/src/mounts.rs): reusable polling thread and mounted-path snapshots. | No SDK watcher. Export Studio's `app/src/main/library.ts` polls `usbList` every two seconds and emits volume events. No equivalent reusable SDK API or rename-stable identity was found. |
| Export inspection | [inspect](crates/rbl-devices/src/lib.rs): track/playlist counts, own-manifest marker and written timestamp. | [rb-device reads](crates/rb-device/src/read.rs) expose richer library records; [rb-usb](crates/rb-usb/src/lib.rs) exposes typed sync inspection/review and explicit recovery. Different contracts, not an identical `DeviceExport` summary API. |
| Drive-hosted master libraries | [libraries](crates/rbl-devices/src/libraries.rs): discover `PIONEER/Master/master.db` and `.PIONEER/Master/master.db` across drives, drive labels/name lookup. | No matching drive-library discovery API found in `rb-device` or the app device handlers. Explicit-path master-library access is a separate `rb-library` capability. |
| Filesystem Explorer | [explorer](crates/rbl-devices/src/explorer.rs): OS roots, subfolders, capped audio listings with totals, hidden/symlink filtering. | No equivalent `rb-device` API. App folder selection and file import are app-only workflows, not the same Explorer contract. |
| Eject | [eject](crates/rbl-devices/src/eject.rs): real-volume validation, macOS diskutil, Linux findmnt/udisksctl, Windows safe-removal/device/media logic. | [rb-device eject](crates/rb-device/src/volumes.rs): listed/ejectable guard and bounded diskutil command. No Linux/Windows counterpart in that implementation. |
| Format | [format](crates/rbl-devices/src/format.rs): macOS-only whole external disk erase, FAT32/MBR or HFS+/GPT; unsupported elsewhere. | [rb-device format](crates/rb-device/src/volumes.rs): guarded macOS diskutil `eraseVolume`, FAT32 only, different label normalization. Different scope and safety checks, not equivalent whole-disk/layout behavior. |
| Library data/import/sync | Inspection/settings helpers use shipped PDB/OneLibrary/manifest codecs. No equivalent import/sync orchestration in the retained crate itself. | `rb-device` exposes tracks/playlists/histories/cues and selection/origin reads. `rb-usb` exposes export/review/recovery; Export Studio's `native/crates/es-engine/src/device_import.rs` and `sync.rs` add app import and sync workflows. These do not replace Explorer, watcher, or discovery APIs. |

## Dependency Context

[upstream-sources.json](upstream-sources.json) still classifies `rbl-devices` as a
test-support crate. No runtime application or `rb-*` dependency on it was added.
[rbl-index](crates/rbl-index/Cargo.toml) still has it as a dev dependency and its
[explorer benchmark example](crates/rbl-index/examples/explorerbench.rs) calls
`rbl_devices::explorer::audio_files`. The retained crate's own tests and examples
also remain. Removal therefore needs explicit feature decisions and updates to
those dev/example dependencies, provenance metadata, documentation and CI excludes;
it is not a feature-equivalent deletion after this settings port alone.