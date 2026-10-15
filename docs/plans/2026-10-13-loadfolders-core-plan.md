# LoadFolders Core and Resolution Implementation Plan

> **For agentic workers:** Use the approved design at `docs/plans/2026-10-13-correctness-loadfolders-design.md`. This plan is one component of the correctness + LoadFolders sprint.

**Goal:** Resolve active-mod-aware folder paths for every input mode, matching RimWorld's LoadFolders conditions and file priority while preserving visible diagnostics.

**Architecture:** Parse LoadFolders into a core model; discover a complete mod set before resolving per-mod paths; pass the active identities into resolution. Keep `Mod`'s public directory-list shape and centralize file-level first-wins selection in a core helper consumed by XML and Harmony detectors.

**Tech Stack:** Rust 2024, existing `quick-xml`, `walkdir`, serde.

**Spec:** `docs/plans/2026-10-13-correctness-loadfolders-design.md`

## Global Constraints

- Do not change `Mod`'s public serialized path fields to per-file lists.
- Keep path traversal rejection and the existing implicit resolver as the fallback for malformed/unselectable LoadFolders data.
- Use existing `FolderResolution` diagnostics; keep report and CLI exit behavior explicit.
- `--config` active IDs come from `ModsConfig.xml`; `--dir`, `--mods`, and FFI use all input mods.
- Only logical commits after focused verification; no push or merge to `main`.

## Review Focus

- A revision-suffixed version from ModsConfig still selects the correct `vX.Y` section.
- Multiple blocks for the same version append entries instead of overwriting earlier ones.
- Multiple recognized condition attributes on one entry combine with AND; the ID lists inside each attribute use their specified matching rule.
- A valid selected section filtered to no folders remains empty; malformed/unselectable LoadFolders falls back with a warning.
- The same relative file in multiple directories keeps only the highest-priority file, deterministically, in every content category.

---

### Task 1: Bounded XML file reader

**Files:** Create `crates/rcd-core/src/xml_io.rs`; modify `crates/rcd-core/src/lib.rs`, `crates/rcd-core/src/about_xml.rs`, `crates/rcd-core/src/mods_config.rs`, and `crates/rcd-core/Cargo.toml` only if required by the existing dependency layout.

**Interfaces:** Provide `pub const MAX_XML_FILE_BYTES: u64 = 50 * 1024 * 1024` and `pub fn read_xml_file(path: &Path) -> std::io::Result<String>`. The implementation must detect growth after metadata inspection (read at most limit+1 bytes) and return `InvalidData` when the ceiling is exceeded.

- [ ] Write tests for exactly-at-limit and over-limit input, and a multibyte UTF-8 file.
- [ ] Run the focused tests; confirm the over-limit case fails before implementation.
- [ ] Implement the helper and route About.xml and ModsConfig.xml reads through it, preserving their current error/reporting behavior.
- [ ] Verify focused tests and existing About.xml/ModsConfig tests pass.

### Task 2: LoadFolders model, parser, and selection

**Files:** Create `crates/rcd-core/src/load_folders.rs`; modify `crates/rcd-core/src/lib.rs` and use `xml_io::read_xml_file` from Task 1.

**Interfaces:** Expose `LoadFolders`, `LoadFolderEntry`, and pure selection/evaluation methods. Keep multiple version blocks in source order and append their children. Parse condition values into `required_any_of`, `required_all_of`, and `disallowed_any_of`; retain unknown condition attribute names for one Info diagnostic per mod.

- [ ] Add failing tests for case-insensitive root/version names, BOM, exact/lower-version/default selection, package-ID/display-name matching (including `_steam` tolerance), comma trimming, `IfModNotActive` none-active semantics, `IfModActiveAll`, and AND across multiple recognized attributes.
- [ ] Add tests for `/`, `\\`, and empty path as root; preserve spaces inside folder names; unknown condition attributes remain unconditional and are recorded for Info.
- [ ] Add tests for malformed XML, over-limit input, duplicate version blocks, and invalid/traversal folder paths.
- [ ] Implement parse and selection. Version normalization must accept strict `X.Y`, `X.Y.BUILD`, and `X.Y.BUILD revN` forms and return only `X.Y`; reject non-numeric/path-like strings.
- [ ] For a malformed file or no applicable version entry, return a FolderResolution Warning and let the caller use implicit resolution. If an applicable block exists but all entries are condition-filtered, return an empty selection plus a Warning.
- [ ] Verify the focused tests pass.

### Task 3: Two-pass resolution and shared file precedence

**Files:** Modify `crates/rcd-core/src/folder.rs`, `crates/rcd-core/src/lib.rs`, and `crates/rcd-cli/src/resolve.rs`; create `crates/rcd-core/src/content_files.rs`; add `walkdir` to `rcd-core` dependencies if needed.

**Interfaces:** `resolve_mod_folders(m: &mut Mod, game_version: Option<&str>, active_mods: &[ModSummary]) -> Vec<Diagnostic>`; add `discover_mods_with_active(search_dirs: &[PathBuf], game_version: Option<&str>, active_ids: &[String]) -> (Vec<Mod>, Vec<Diagnostic>)` while retaining a compatible discover-all wrapper if current consumers require it. Provide `pub fn collect_files_with_precedence(dirs: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>)`.

- [ ] Add tests proving `--dir` uses all discovered mods as active, `--mods` uses all supplied mods, and `--config` uses only activeMods when resolving conditions.
- [ ] Add folder tests for root + version + Common coexistence, explicit LoadFolders precedence (reverse source order so the last eligible entry wins), missing directories, malformed-file fallback, no-match fallback, and selected-but-empty behavior.
- [ ] Add `collect_files_with_precedence` tests for nested relative paths, duplicate path first-wins, case-sensitive distinct paths, and deterministic ordering.
- [ ] Refactor discovery to parse the full mod set before resolving folders. Resolve `--config` candidates using activeMods before filtering output; then retain only active mods as today.
- [ ] Always include the root content directories after version/Common directories in implicit mode; keep 1.0 on implicit resolution.
- [ ] Add a shared base-content helper in `rcd-core` and use it from every CLI input mode.
- [ ] Map LoadFolders failures to Warning diagnostics and preserve the mod in `mods_scanned`; malformed/unselectable data gets implicit paths, while a valid all-filtered selection keeps no paths.
- [ ] Verify core and CLI focused tests pass.

### Task 4: FFI resolution and config contract

**Files:** Modify `crates/rcd-ffi/src/lib.rs`; use the core resolver and shared base-content helper from Task 3.

**Interfaces:** Add typed `RcdConfig` with defaults for omitted `mod_paths`, `game_version`, and `ordered`; retain unknown keys in an extras map and convert them to a `MetadataResolution` Warning in the report. Invalid known-field types return the existing JSON error envelope.

- [ ] Add failing tests for revision-suffixed game_version, active-set conditional resolution from all mod_paths, FFI `is_base_content`, unknown-key warning-and-continue, and invalid type errors.
- [ ] Document `ProgressCallback.phase` as valid only for the duration of the synchronous callback invocation.
- [ ] Implement typed deserialization and route all parsed mods through shared folder and base-content resolution.
- [ ] Verify FFI tests and crate-level tests pass.

### Task 5: Integrated sprint verification (parent-owned; after all three plans)

**Files:** No source changes unless a failing gate requires a scoped fix.

- [ ] Run `just ci` on the integrated feature branch and resolve failures before claiming completion.
- [ ] Run the CLI against `/mnt/bulk/anten/.local/share/Steam/steamapps/workshop/content/294100`; capture conflict counts and diagnostics, compare with a baseline run, and investigate new diagnostic/conflict classes for false positives.
- [ ] If the corpus is inaccessible or a gate cannot run, record it as unverified with the exact blocker; do not claim a full validation pass.
- [ ] Have a fresh read-only reviewer inspect the integrated diff and verify every change maps to the approved design and one of these plans.
