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
- `rb-device`: mounted volume discovery and device-library reads.
- `rb-anlz`: ANLZ file access and parsing helpers.
- `rb-usb`: typed inspection, export, and explicit recovery service.
- `rbl-*`: tested format codecs, database builders/readers, and export engine.

The `rbl-db`, `rbl-index`, `rbl-analysis`, and `rbl-devices` crates support
upstream-derived tests and examples; they are not production dependencies of the
SDK facade.

## Build And Test

Requires Rust 1.89 or newer.

```sh
cargo check --workspace --locked
cargo test --workspace --locked
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

## License

GPL-2.0-or-later. See [LICENSE](LICENSE), [COPYING](COPYING), and
[licenses/rbxport](licenses/rbxport) for the applicable terms and retained source
licenses. Derived rbxport code remains subject to its corresponding-source
obligations.
