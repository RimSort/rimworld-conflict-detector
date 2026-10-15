# Task 2 implementation report: LoadFolders model, parser, and selection

## Interfaces supplied for Task 3

- `rcd_core::load_folders::LoadFolders::{parse_file, parse_str, select, resolve_file}` parses a path or XML text, selects against an optional game version and active identities, or turns file read/parse failures into a `FolderResolution` Warning with implicit-fallback semantics.
- `LoadFolderEntry` exposes original `path` text, `required_any_of`, `required_all_of`, and `disallowed_any_of`; `matches(&[ModIdentity])` evaluates the conditions and `relative_path()` returns the validated mod-relative `PathBuf` (empty path represents the mod root).
- `ModIdentity` supplies `package_id` and `display_name`; matches are case-insensitive and tolerate `_steam` on either package-ID spelling.
- `LoadFolders` retains `default_entries`, source-ordered `version_entries`, duplicate-block child order, and deduplicated source-ordered `unknown_conditions` for a later once-per-mod Info diagnostic.
- `FolderSelection.entries == None` means warn and use implicit resolution; `Some(vec![])` is the deliberate selected-empty outcome and remains distinguishable. Warnings use existing `DiagnosticSource::FolderResolution`.

## Changes

- Added `crates/rcd-core/src/load_folders.rs` and exported it from `crates/rcd-core/src/lib.rs`.
- Reads files through shared `xml_io::read_xml_file`; no duplicate size bound was added.
- Added six focused unit tests covering case-insensitive root/version lookup, BOM, duplicate blocks, exact/lower/default selection, strict version normalization, condition parsing/matching/AND semantics, root markers and text preservation, unknown attributes, invalid paths, malformed/no-version/empty-selection distinctions, warning source, and over-limit file rejection.
- No discovery, CLI, FFI, folder integration, plans/design, or unrelated tracked files were changed.

## TDD evidence

- RED: `cargo test -p rcd-core --lib load_folders -- --nocapture` initially compiled and ran the new tests; 5 passed and `malformed_and_no_version_are_distinguishable_from_empty_selected_result` failed because `<loadFolders>` without its closing root was accepted.
- GREEN: after strict root closure validation and implementation fixes, the same focused command passed all 6 tests.

## Verification

- `cargo test -p rcd-core --lib load_folders -- --nocapture` — passed, 6 passed.
- `cargo test --workspace` — passed across the workspace (76 rcd-xml tests, 48 rcd-core unit tests including new LoadFolders tests, and all other workspace suites; one pre-existing ignored test).
- `cargo clippy --workspace --all-targets -- -D warnings` — passed.
- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.
- `cargo test -p rcd-core load_folders -- --nocapture` was also attempted as the initial focused RED command; cargo's integration test target lacked `test-helpers`, so this invocation failed for that unrelated feature-gating reason before the unit tests ran. The `--lib` invocation is the successful focused test path.

## Commit and state

Implementation source commit: `df3de34` (`feat(core): add LoadFolders parsing and selection`). The report artifact was updated and committed separately after the source commit.
No push or merge was performed. Pre-existing untracked `docs/plans/*` files were preserved untouched. No staged files should remain after committing.

## Concerns / boundaries

- Task 3 must attach the owning mod identity to diagnostics if desired and turn the retained `unknown_conditions` into its once-per-mod Info diagnostic; this module deliberately does not integrate discovery or emit that Info itself.
- Path selection rejects parent traversal and absolute paths (including Windows drive prefixes on non-Windows hosts); accepted original folder text is retained while `relative_path()` normalizes separators.
- Full behavior of root-level filename discovery and implicit fallback ordering is intentionally deferred to Task 3.
