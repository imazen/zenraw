# Metadata API audit

## zenraw, 2026-10-12

The published baseline is crates.io zenraw 0.2.0. Structural inventory is new
relative to that release. Its implementation reuses PR 20 (`3f8bc711`) with the
shared IFD implementation in zenmeta instead of zenifd. RAW backend consumption
rules remain private to zenraw. The new `inventory` feature is off by default;
`zencodec` alone does not enable zenmeta. No public signature exposes zenmeta.
The existing config/job associated error types remain `At<RawError>`; compile
constraints in `tests/inventory_feature.rs` guard them separately from semver.

`just metadata-semver` compares the adapter and default native API against the
published release. It reports two existing classes of break: removed
`RawError::{Decode, InvalidInput, Unsupported}` variants and an additional field
on `RawError::LimitExceeded`. They predate this extraction, are recorded in the
queued-breaking changelog, and `src/error.rs` is unchanged by the extraction.
Do not call this tree compatible with 0.2.0 or erase that release requirement.

The consumer temporarily pins the zencodec/zenmeta review revision directly.
Replace those pins with released dependencies before publishing. A workspace
patch alone does not propagate to external consumers.

## Validation and limits

`just metadata-check` exercises both rawloader and rawler inventory rules,
pregate structural fixtures, input-limit/cancellation handling, dynamic dispatch,
the disabled feature, and saved fuzz regressions. The two additional minimized
inputs are exact bytes from PR 20 represented as hex text; the existing corpus
floor is unchanged. `just metadata-lint` checks all targets/features and
`just metadata-fuzz-check` compiles every fuzz target.

The independent ExifTool check used six RAW samples (CR2, DNG, NEF, ORF, RW2,
ARW), comparing 550 directory/value units for each backend with no unexplained
mismatch. The RW2 file has 65 explained units within an opaque embedded TIFF.
This verifies structural offsets, not every decoder-consumption claim. The
existing mutation tests check selected consumed/unread parts. CR3 and RAF have
synthetic structural tests but were not part of this ExifTool run.

Inventory runs the backend's pre-pixel container check under its existing panic
isolation. rawler failures that occur in `raw_image` before pixel work are not
covered: calling that method with its dummy flag still decodes DNG pixels.
Budget exhaustion leaves unknown ranges; it does not certify them unused.
This inventory is not a deletion recipe.

`just metadata-i686-check` passes both backend suites on i686, including the
same configured input-limit and cancellation details through dynamic dispatch.
`just metadata-minimal-check` checks inventory with no backend and without std.
CI covers the enabled/disabled contract and both backends, including i686.
