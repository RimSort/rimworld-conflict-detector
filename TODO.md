# RimWorld Conflict Detector — TODO

## Tuning (from real-world testing)

- [x] **Same-mod def replacement false positives** — When a mod has version-specific folders (e.g., `1.5/Defs/` and `Common/Defs/`), both may provide the same def. Currently flagged as a conflict between the mod and itself. Suppress when all involved mods have the same package_id.

- [x] **Wildcard XPath overlap severity** — XPaths without a `defName` predicate (e.g., `Defs/ThingDef/statBases`) match all defs of that type, generating massive warning counts (~14k+ for a 739-mod list). Downgrade wildcard-vs-specific overlaps to `Info` severity. Consider wildcard-vs-wildcard as `Info` too.

## Review Findings (minor, from adversarial review)

- [x] **`--color` flag** — Parsed but never used. Add ANSI color output with a terminal coloring crate (e.g., `termcolor` or `owo-colors`). Red for ERROR, yellow for WARN, gray for INFO.

- [x] **Two transpilers = Info instead of Warning** — Multiple transpilers on the same method from different mods rarely compose correctly. Should be at least Warning severity.

- [x] **Abstract def skip diagnostic** — Spec requires an Info-level diagnostic noting how many abstract defs were skipped per mod. Currently skipped silently.

- [x] **Non-overlapping same-def XPath Info conflicts** — Emit aggregated `Info` conflicts for non-overlapping XPath patches on the same def, retaining the XPath pairs.

- [x] **Manual Harmony patch detection not integrated** — `manual.rs` with `detect_manual_patches()` exists and has tests, but `HarmonyDetector::analyze` never calls it. Wire it in, flag results as `Info` with best-effort disclaimer.

- [x] **`PatchOperationConditional` not handled** — Falls through to wildcard skip. Should parse the XPath test and execute match/nomatch children similar to `PatchOperationFindMod`.

- [ ] **`PatchOperationSequence` success attribute** — Parsed but the `success` attribute is not read or used. For static analysis the safer direction is to assume all operations execute, but documenting the limitation would help.

- [x] **Shared `is_base_content` classification** — The `ludeon.*` heuristic now marks Core/DLC mods across `--config`, `--dir`, `--mods`, and FFI (`rcd_analyze`) resolution paths.

- [x] **FFI timestamp empty** — `rcd_analyze` sets `timestamp: String::new()`. Should use a real ISO 8601 timestamp (add chrono as an rcd-ffi dependency or pass it through the config JSON).

- [x] **Phase 2 conflict detection not parallelized** — Spec calls for parallel pairwise comparison with rayon. Currently sequential. Low priority since it's fast on in-memory data.

- [x] **`error_json` incomplete JSON escaping** — Manually escapes quotes and newlines but misses backslashes and control characters. Replace with `serde_json::json!({"error": msg}).to_string()`.

## Robustness

- [x] **Silent file I/O errors** — `def_parser`, `patch_ops`, `discover_assemblies` all use `.filter_map(|e| e.ok())` on walkdir iterators and `continue` on read failures. Should propagate as diagnostics.

- [ ] **`defName` parser fragility** — Assumes the next event after `<defName>` opening tag is `Event::Text`. Comments or CDATA would cause silent failure. Worse: empty elements (`<defName></defName>`) cause parser desynchronization that swallows all subsequent sibling defs/operations (see review finding above). *(defName leg fixed on `fix/p1-error-propagation`: loop consumes End, CDATA/comments handled, nested markup diagnosed; xpath leg still open — `read_xpath_from_operation` in `patch_ops.rs` retains the one-shot Text-then-discard-End idiom)*

- [ ] **No file size limits** — All file reads are unbounded. Add a reasonable max (e.g., 50MB) as a defensive measure.

- [ ] **No XML recursion depth limit** — `parse_operation`, `parse_sequence`, `parse_find_mod` are mutually recursive. Deeply nested sequences could exhaust the stack (~16k levels needed, unlikely in practice).

## Architecture

- [ ] **About.xml two-pass parser** — Parses XML twice (section-level fields, then top-level scalars). Could be consolidated to a single pass.

- [x] **`HashMap<String, bool>` in Report::build** — Should be `HashSet<String>`. The bool value is never read.

- [ ] **Inconsistent error types** — `mods_config.rs` returns `Box<dyn Error>`, `about_xml.rs` uses typed errors with thiserror, `HarmonyParseError` manually implements Display/Error. Standardize.

- [x] **`ModDefs.package_id` duplicates `ModSummary.package_id`** — Redundant field. Use `mod_summary.package_id` directly.

- [x] **Spike test still in lib.rs** — ~470-line exploratory test with 43 println! statements. The focused tests in `attributes.rs` and `priority.rs` cover the same scenarios. Remove or move to examples/.

- [x] **Duplicated test helpers** — `test_mod()` and `fixtures_path()` copied across 5+ test modules. Consider a shared `rcd-test-utils` crate or a `tests/common/` module.

- [x] **Glob re-export of conflict module** — `pub use conflict::*` in rcd-core lib.rs unlike all other module re-exports which use explicit item lists.

## Security (low risk, local tool)

- [x] **Path traversal via `game_version`** — Used without validation in path construction. Validate against `r"^[0-9]+\.[0-9]+$"`.

## Codebase Review Findings (Opus review, 2025-08)

### Correctness

- [ ] **XPathOnReplacedDef `break` causes false negatives** — `crates/rcd-xml/src/conflict.rs:128`. Loop over `replacing_mods` breaks after the first match. If 3+ mods define the same def and a 4th patches it, only one conflict is reported instead of all pairs. The `break` is unnecessary (map is already deduplicated) and incorrect. Remove it.

- [ ] **FFI `is_base_content` still not set** — `crates/rcd-ffi/src/lib.rs:54`. The CLI-side fix for this (marked `[x]` above) only covers `resolve.rs`. The FFI calls `parse_about_xml` + `resolve_mod_folders` directly without the `ludeon.*` heuristic. C# consumers always get `is_base_content: false` for Core/DLC mods.

- [ ] **Raw XML reconstruction drops root def attributes** — `crates/rcd-xml/src/def_parser.rs` (`parse_single_def`, raw_xml reconstruction). `format!("<{def_type}>{inner_xml}</{def_type}>")` discards `Abstract`, `ParentName`, `Name` attributes from the opening tag. Two defs with identical inner content but different inheritance (e.g., one abstract, one concrete) are incorrectly considered identical, suppressing real replacement conflicts.

- [ ] **Empty `<defName>`/`<xpath>` causes parser desynchronization** — `parse_single_def`'s defName leaf block and `patch_ops.rs` (`read_xpath_from_operation`). For `<defName></defName>`, the first read returns `Event::End` (not Text), then the second read consumes the *next* event — swallowing subsequent sibling defs or patch operations. Extends the "defName parser fragility" item above — the consequence is not just silent failure of one def, but cascading desync that loses all subsequent siblings. *(defName leg fixed on `fix/p1-error-propagation`; xpath leg still open. Gate-review correction: "loses all subsequent siblings" was overstated — damage was in-def only, the outer loop resyncs at the next def `Start`)*

### Error Handling

- [x] **`discover_mods` silently swallows I/O and parse errors** — `crates/rcd-core/src/folder.rs:72-139`. Uses `let Ok(entries) = ... else { continue }` and `if let Ok(mut mod_data) = parse_about_xml(...)`. Callers can't distinguish "no mods found" from "all mods failed to parse." Should return `(Vec<Mod>, Vec<Diagnostic>)` consistent with `parse_defs_dir`/`parse_patches_dir`.

- [x] **FFI silently drops mods that fail to parse** — `crates/rcd-ffi/src/lib.rs:53-66,101`. Unlike the CLI which emits Error diagnostics for failed mods, the FFI boundary skips them with no visibility. Failed mods should be collected and included in `report.diagnostics`.

- [x] **XML parse errors silently ignored in def/patch parsers** — formerly `def_parser.rs:109,180` and `patch_ops.rs:113,204,246,289,332,372,407` (pre-fix positions). Used `Err(_) => break` to silently ignore XML parse errors, returning partial results. Malformed XML containing conflicting defs produces false negatives. *Fixed on `fix/p1-error-propagation`: every loop site now captures via a threaded errors vec, plus a `check_well_formed` pre-check per entry point for errors consumed by nested best-effort reads.*

- [x] **Manual patch detection panics not reported as diagnostics** — `crates/rcd-harmony/src/lib.rs:93-118`. `catch_unwind` catches the panic but emits no diagnostic, unlike `parse_harmony_patches` (lines 60-82) which emits a Warning. Should use `match` instead of `if let Ok` to emit a diagnostic on `Err`.

### Performance

- [x] **O(n²) XPath conflict detection with redundant normalization** — `crates/rcd-xml/src/conflict.rs:34-45`. Nested loop compares every pair of patch operations + calls `normalize_xpath` inside the inner loop for each pair. For 739 mods × ~10 patches = ~27M comparisons. Should group patches by `(def_type, def_name)` and pre-normalize XPaths once, reducing to O(n·k) where k is avg patches per def.

- [x] **`LoadOrder::position` is O(n) linear scan in inner loops** — `crates/rcd-core/src/types.rs:48`. Called twice per overlapping pair in `detect_xpath_conflicts`, making winner determination O(n³) worst case. Should use `HashMap<String, usize>` for O(1) lookups.

### Architecture / API

- [x] **`resolve_mod_folders` active-mod input** — Folder resolution now receives active mod identities for `LoadFolders.xml` condition evaluation.

- [ ] **`pub mod` leaks implementation details in rcd-xml and rcd-harmony** — Both crates declare all submodules as `pub mod`, exposing internal types (`ModDefs`, `ParsedDef`, `DiscoveredPatch`, etc.) when only `XmlDetector`/`HarmonyDetector` are consumed externally. Should be `mod` with selective re-exports.

- [x] **FFI progress callback `phase` pointer lifetime documented** — `crates/rcd-ffi/src/lib.rs:8` documents that the pointer is valid only during the synchronous callback invocation; consumers must copy the string if they need to retain it.

- [x] **Typed FFI configuration validation** — `crates/rcd-ffi/src/lib.rs:11-21` defines `RcdConfig` with `#[derive(Deserialize)]`. Invalid types for known fields error, unknown fields produce a Warning, and omitted fields retain their defaults.

## Follow-ups (final-review backlog)

Items surfaced by the final cross-commit review and its gate reviews, deliberately not fixed in `fix/p1-error-propagation` to keep the branch review-sized:

- [ ] **`patch_ops.rs` nested one-shot reads** — `read_xpath_from_operation`/`read_li_text_list` keep the Text-then-discard-End idiom the defName loop fixed; same desync/silent-drop class, in-def damage only.
- [ ] **`replacement.rs` undecodable-text silent drops** — `normalize_whitespace` and `extract_tag_content`'s text arm discard `unescape()` errors; field content can vanish from the diff silently (pre-existing pattern).
- [ ] **Comment/PI not re-emitted into `raw_xml`** — def_parser skips them from the reconstruction (matches pre-existing outer-loop behavior; only affects byte-fidelity of raw_xml, not identity).
- [ ] **T1 manual-scan Err-arm test seam** — the `catch_unwind` Err→Warning arm in HarmonyDetector is verified by inspection only; a crash fixture was declined as YAGNI.
- [x] **`list-mods --format json` diagnostics envelope** — JSON output includes both discovered mods and resolution diagnostics, including total failure.
- [x] **Concrete-def-no-defName diagnostic may need a config-file escape hatch** — resolved by aggregation: a corpus run over 1573 workshop mods showed the per-def Warning firing on vanilla Core/Royalty SongDefs (legitimate — RimWorld auto-names defName-less defs at runtime). Nameless concrete defs now aggregate into one Info diagnostic per mod, matching the abstract-skip convention; no escape hatch needed.

## Deferred Features (from spec)

- [ ] **C# RimWorld mod** — P/Invoke into rcd-ffi, `[StaticConstructorOnStartup]` lifecycle, in-game UI with Verse widgets, DllMap for cross-platform native library loading. Separate implementation plan needed.

- [ ] **RimSort Steam Workshop Database fallback** — Query RimSort's community database for `packageId → workshopId` mapping when local resolution fails.

- [x] **`LoadFolders.xml` conditional evaluation** — Parse supported active/inactive condition attributes and evaluate them against active mod identities for folder resolution. Unknown condition attributes are surfaced as informational diagnostics.

- [ ] **Field-level XML tree diffing** — Current replacement detection does whole-def comparison. A more sophisticated implementation would diff individual child elements and report specific divergent fields more accurately.

- [ ] **Def inheritance conflicts (v2)** — Mods overriding or extending abstract base defs via `ParentName`. Requires building the def inheritance tree.

- [ ] **ModExtension conflicts (v2)** — Mods attaching `DefModExtension` data to existing defs.

- [ ] **Additional PatchOperation types (v2)** — `PatchOperationAttributeSet`, `PatchOperationAttributeRemove`, `PatchOperationSetName`, `PatchOperationAddModExtension`, `PatchOperationTest`.

- [ ] **Pre-built conflict database** — Curated/crowdsourced database of known mod conflicts for instant results without scanning.

- [ ] **Native app integration** — Embedding the Rust library in a standalone web or desktop application.

- [ ] **Community Rules Database Rule Generator** - Generate community db rules from select mods.

- [ ] **RimSort Integration** - TBD
