# es-rekordbox-sdk

A Rust workspace for reading and writing Rekordbox libraries and exported USB
volumes. Export Studio is the first consumer; the SDK packages expose generic
records and services without Electron, N-API, application sessions, or UI state.

This repository contains the tested `rbl-*` format/export implementation and the
application-independent `rb-*` APIs layered over it. It does not contain a CLI.

## Crates

- `rb-model`: shared source records, conflict diagnostics, decisions, and plans.
- `rb-storage`: durable filesystem publication and recovery primitives.
- `rb-library`: guarded `master.db` access and transactions.
- `rb-device`: mounted volume discovery, device-library reads, and device settings.
- `rb-anlz`: ANLZ file access and parsing helpers.
- `rb-audio`: where rekordbox's timeline starts in an audio file, read from the file alone.
- `rb-usb`: typed inspection, export, and explicit recovery service.
- `rbl-*`: tested format codecs, database builders/readers, and export engine.

The `rbl-db`, `rbl-index`, `rbl-analysis`, and `rbl-devices` crates are retained
because upstream-derived tests and examples for shipped crates use their
fixtures and helper implementations. They are not user-facing SDK capabilities,
and their standalone suites are excluded from standard SDK validation.

## Build And Test

Requires Rust 1.89 or newer.

```sh
cargo check --workspace --locked --exclude rbl-analysis --exclude rbl-db \
	--exclude rbl-devices --exclude rbl-index
cargo test --workspace --locked --exclude rbl-analysis --exclude rbl-db \
	--exclude rbl-devices --exclude rbl-index
node check-layers.mjs
node check-upstream.mjs /path/to/rbxport-at-pinned-commit
```

## USB Safety Contract

`rb_usb::inspect` is read-only and does not recover an interrupted publication.
Inspection returns typed diagnostics, field evidence, supported resolutions, and
a token bound to the current request and observed device state. Export revalidates
the review while holding the publication lock. Pending publication recovery is an
explicit operation. Unsupported schemas, integrity failures, ownership or identity
uncertainty, unresolved history references, and unsafe cross-format differences
remain blockers; there is no universal force action.

Manifest v3 stores accepted review decisions with the recoverable generation.
Versions 1 and 2 remain readable. See [UPSTREAM.md](UPSTREAM.md) for the pinned
source baseline, local derivations, and license provenance.

## Device Settings

`rb_device::settings::read(volume)` reads display settings from `DEVSETTING.DAT`,
OneLibrary browse categories/sort order/sub-column, device name and color labels,
and the Device Library background from `export.pdb`. Missing groups are `None`;
damaged or unsupported files are errors, not defaults to overwrite. Both
`PIONEER` and `.PIONEER` roots are supported. The file APIs are platform-independent;
automatic volume discovery/eject/format in `rb-device` currently use macOS tooling.

Clone the returned `DeviceSettings`, edit supported fields, then call
`write_changes(volume, &original, &edited)`. It rejects stale settings under the
exclusive device lease and publishes only changed groups. Unrecognized display
bytes are preserved; unknown display enum values are refused. `write` is also
available for callers that intentionally replace the latest settings without an
earlier snapshot. Neither writer creates a library or deletes absent groups.
Browse settings require an existing OneLibrary. Device name and color labels are
mirrored to an existing Device Library; unsupported PDB rewrites fail atomically.

Reads are read-only and refuse pending publication. Use `rb_usb::recover(volume)`
explicitly before retrying. Settings and exports share its recovery journal and
storage lease. Callers must also prevent external editors (including rekordbox)
from writing concurrently; these leases serialize SDK operations, not rekordbox.
Waveform divisions and MYSETTING/MYSETTING2/DJMMYSETTING player preferences are not
implemented. This ports the supported settings surface from retained
`rbl-devices`; it does not remove that crate or complete the inventory in #5.
See [DEVICE_SETTINGS.md](DEVICE_SETTINGS.md) for the source-linked remaining
differences and dev/example dependency context.

## License

GPL-2.0-or-later. See [LICENSE](LICENSE), [COPYING](COPYING), and
[licenses/rbxport](licenses/rbxport) for the applicable terms and retained source
licenses. Derived rbxport code remains subject to its corresponding-source
obligations.
