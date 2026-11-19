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

- [x] **`PatchOperationSequence` success attribute** — Static analysis conservatively assumes all contained operations may execute. The README documents that runtime `success` behavior is not evaluated.

- [x] **Shared `is_base_content` classification** — The `ludeon.*` heuristic now marks Core/DLC mods across `--config`, `--dir`, `--mods`, and FFI (`rcd_analyze`) resolution paths.

- [x] **FFI timestamp empty** — `rcd_analyze` sets `timestamp: String::new()`. Should use a real ISO 8601 timestamp (add chrono as an rcd-ffi dependency or pass it through the config JSON).

- [x] **Phase 2 conflict detection not parallelized** — Spec calls for parallel pairwise comparison with rayon. Currently sequential. Low priority since it's fast on in-memory data.

- [x] **`error_json` incomplete JSON escaping** — Manually escapes quotes and newlines but misses backslashes and control characters. Replace with `serde_json::json!({"error": msg}).to_string()`.

## Robustness

- [x] **Silent file I/O errors** — `def_parser`, `patch_ops`, `discover_assemblies` all use `.filter_map(|e| e.ok())` on walkdir iterators and `continue` on read failures. Should propagate as diagnostics.

- [ ] **Investigate XML leaf-text edge cases** — Empty values, comments, and CDATA are handled by the leaf parsers, but source inspection suggests that trimming text events before concatenation may remove whitespace around comments or CDATA. A nested element with the same name as its enclosing leaf may also cause `read_element_text` to return early. Add focused reproductions before determining impact or fixes; these cases have not been verified.

- [ ] **Unbounded input and report memory** — XML and managed DLL reads and generated reports are intentionally uncapped; large inputs or many findings can exhaust memory. Consider a future streaming design without imposing size caps.

- [x] **Patch-operation recursion limit** — Operations nested beyond 64 levels are diagnosed and skipped iteratively so later siblings can still be analyzed. This limits patch-operation nesting, not general XML depth.

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

- [x] **XPathOnReplacedDef reports each replacing mod** — The loop over `replacing_mods` no longer stops after the first match, so each applicable replacement can produce a conflict.

- [x] **FFI `is_base_content` classification** — `rcd_analyze` calls the shared `mark_base_content` helper before building active-mod metadata; an FFI test checks the Core flag in the report.

- [x] **Root def attributes in raw XML** — Reconstruction retains root attributes, and replacement comparison includes attribute differences.

- [x] **Empty `<defName>`/`<xpath>` parser desynchronization** — The leaf readers consume their closing elements without the old one-shot Text-then-End read. Empty values, comments, CDATA, and following siblings have focused tests; additional possible edge cases are tracked under Robustness.

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

- [x] **`patch_ops.rs` nested one-shot reads** — XPath and list leaves now use `read_element_text` instead of the Text-then-discard-End idiom. Possible remaining edge cases are tracked under Robustness.
- [x] **`replacement.rs` undecodable-text handling** — Normalization and field extraction diagnose text-decode failures; invalid input is retained as a marker rather than silently dropped or treated as equivalent.
- [ ] **Comments/PIs not re-emitted into `raw_xml`** — The def parser omits them during reconstruction. Investigate the effect on text boundaries and comparison semantics rather than assuming the difference only affects byte fidelity.
- [ ] **T1 manual-scan Err-arm test seam** — the `catch_unwind` Err→Warning arm in HarmonyDetector is verified by inspection only; a crash fixture was declined as YAGNI.
- [x] **`list-mods --format json` diagnostics envelope** — JSON output includes both discovered mods and resolution diagnostics, including total failure.
- [x] **Concrete-def-no-defName diagnostic may need a config-file escape hatch** — resolved by aggregation: a corpus run over 1573 workshop mods showed the per-def Warning firing on vanilla Core/Royalty SongDefs (legitimate — RimWorld auto-names defName-less defs at runtime). Nameless concrete defs now aggregate into one Info diagnostic per mod, matching the abstract-skip convention; no escape hatch needed.

## Deferred Features (from spec)

- [ ] **C# RimWorld mod** — P/Invoke into rcd-ffi, `[StaticConstructorOnStartup]` lifecycle, in-game UI with Verse widgets, DllMap for cross-platform native library loading. Separate implementation plan needed.

- [ ] **RimSort Steam Workshop Database fallback** — Query RimSort's community database for `packageId → workshopId` mapping when local resolution fails.

- [x] **`LoadFolders.xml` conditional evaluation** — Parse supported active/inactive condition attributes and evaluate them against active mod identities for folder resolution. Unknown condition attributes are surfaced as informational diagnostics.

- [ ] **Field-level XML tree diffing** — Replacement detection already reports differing root attributes and uses a heuristic for top-level child tags. A more sophisticated tree diff could report nested differences more precisely.

- [ ] **Def inheritance conflicts (v2)** — Mods overriding or extending abstract base defs via `ParentName`. Requires building the def inheritance tree.

- [ ] **ModExtension conflicts (v2)** — Mods attaching `DefModExtension` data to existing defs.

- [ ] **Additional PatchOperation types (v2)** — `PatchOperationAttributeSet`, `PatchOperationAttributeRemove`, `PatchOperationSetName`, `PatchOperationAddModExtension`, `PatchOperationTest`.

- [ ] **Pre-built conflict database** — Curated/crowdsourced database of known mod conflicts for instant results without scanning.

- [ ] **Native app integration** — Embedding the Rust library in a standalone web or desktop application.

- [ ] **Community Rules Database Rule Generator** - Generate community db rules from select mods.

- [ ] **RimSort Integration** - TBD
