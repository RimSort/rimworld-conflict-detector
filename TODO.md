# RimWorld Conflict Detector — TODO

## Tuning (from real-world testing)

- [x] **Same-mod def replacement false positives** — When a mod has version-specific folders (e.g., `1.5/Defs/` and `Common/Defs/`), both may provide the same def. Currently flagged as a conflict between the mod and itself. Suppress when all involved mods have the same package_id.

- [ ] **Wildcard XPath overlap severity** — XPaths without a `defName` predicate (e.g., `Defs/ThingDef/statBases`) match all defs of that type, generating massive warning counts (~14k+ for a 739-mod list). Downgrade wildcard-vs-specific overlaps to `Info` severity. Consider wildcard-vs-wildcard as `Info` too.

## Review Findings (minor, from adversarial review)

- [ ] **`--color` flag** — Parsed but never used. Add ANSI color output with a terminal coloring crate (e.g., `termcolor` or `owo-colors`). Red for ERROR, yellow for WARN, gray for INFO.

- [ ] **Two transpilers = Info instead of Warning** — Multiple transpilers on the same method from different mods rarely compose correctly. Should be at least Warning severity.

- [ ] **Abstract def skip diagnostic** — Spec requires an Info-level diagnostic noting how many abstract defs were skipped per mod. Currently skipped silently.

- [ ] **Non-overlapping same-def XPath Info conflicts** — Spec says XPath patches on the same def but non-overlapping nodes should be `Info`. Currently not emitted at all (deliberate simplification, but spec-noncompliant).

- [ ] **Manual Harmony patch detection not integrated** — `manual.rs` with `detect_manual_patches()` exists and has tests, but `HarmonyDetector::analyze` never calls it. Wire it in, flag results as `Info` with best-effort disclaimer.

- [ ] **`PatchOperationConditional` not handled** — Falls through to wildcard skip. Should parse the XPath test and execute match/nomatch children similar to `PatchOperationFindMod`.

- [ ] **`PatchOperationSequence` success attribute** — Parsed but the `success` attribute is not read or used. For static analysis the safer direction is to assume all operations execute, but documenting the limitation would help.

- [ ] **`is_base_content` only set in `--config` mode** — `--dir` and `--mods` modes never mark Core/DLC mods. Apply the `ludeon.*` heuristic in all resolution paths.

- [ ] **FFI timestamp empty** — `rcd_analyze` sets `timestamp: String::new()`. Should use a real ISO 8601 timestamp (add chrono as an rcd-ffi dependency or pass it through the config JSON).

- [ ] **Phase 2 conflict detection not parallelized** — Spec calls for parallel pairwise comparison with rayon. Currently sequential. Low priority since it's fast on in-memory data.

- [ ] **`error_json` incomplete JSON escaping** — Manually escapes quotes and newlines but misses backslashes and control characters. Replace with `serde_json::json!({"error": msg}).to_string()`.

## Robustness

- [ ] **Silent file I/O errors** — `def_parser`, `patch_ops`, `discover_assemblies` all use `.filter_map(|e| e.ok())` on walkdir iterators and `continue` on read failures. Should propagate as diagnostics.

- [ ] **`defName` parser fragility** — Assumes the next event after `<defName>` opening tag is `Event::Text`. Comments or CDATA would cause silent failure.

- [ ] **No file size limits** — All file reads are unbounded. Add a reasonable max (e.g., 50MB) as a defensive measure.

- [ ] **No XML recursion depth limit** — `parse_operation`, `parse_sequence`, `parse_find_mod` are mutually recursive. Deeply nested sequences could exhaust the stack (~16k levels needed, unlikely in practice).

## Architecture

- [ ] **About.xml two-pass parser** — Parses XML twice (section-level fields, then top-level scalars). Could be consolidated to a single pass.

- [ ] **`HashMap<String, bool>` in Report::build** — Should be `HashSet<String>`. The bool value is never read.

- [ ] **Inconsistent error types** — `mods_config.rs` returns `Box<dyn Error>`, `about_xml.rs` uses typed errors with thiserror, `HarmonyParseError` manually implements Display/Error. Standardize.

- [ ] **`ModDefs.package_id` duplicates `ModSummary.package_id`** — Redundant field. Use `mod_summary.package_id` directly.

- [ ] **Spike test still in lib.rs** — ~470-line exploratory test with 43 println! statements. The focused tests in `attributes.rs` and `priority.rs` cover the same scenarios. Remove or move to examples/.

- [ ] **Duplicated test helpers** — `test_mod()` and `fixtures_path()` copied across 5+ test modules. Consider a shared `rcd-test-utils` crate or a `tests/common/` module.

- [ ] **Glob re-export of conflict module** — `pub use conflict::*` in rcd-core lib.rs unlike all other module re-exports which use explicit item lists.

## Security (low risk, local tool)

- [ ] **Path traversal via `game_version`** — Used without validation in path construction. Validate against `r"^[0-9]+\.[0-9]+$"`.

- [ ] **No XML depth limit** — See robustness section above.

## Deferred Features (from spec)

- [ ] **C# RimWorld mod** — P/Invoke into rcd-ffi, `[StaticConstructorOnStartup]` lifecycle, in-game UI with Verse widgets, DllMap for cross-platform native library loading. Separate implementation plan needed.

- [ ] **RimSort Steam Workshop Database fallback** — Query RimSort's community database for `packageId → workshopId` mapping when local resolution fails.

- [ ] **`LoadFolders.xml` conditional evaluation** — Parse `IfModActive`/`IfModNotActive` conditions and evaluate against the active mod set for folder resolution.

- [ ] **Field-level XML tree diffing** — Current replacement detection does whole-def comparison. A more sophisticated implementation would diff individual child elements and report specific divergent fields more accurately.

- [ ] **Def inheritance conflicts (v2)** — Mods overriding or extending abstract base defs via `ParentName`. Requires building the def inheritance tree.

- [ ] **ModExtension conflicts (v2)** — Mods attaching `DefModExtension` data to existing defs.

- [ ] **Additional PatchOperation types (v2)** — `PatchOperationAttributeSet`, `PatchOperationAttributeRemove`, `PatchOperationSetName`, `PatchOperationAddModExtension`, `PatchOperationTest`.

- [ ] **Pre-built conflict database** — Curated/crowdsourced database of known mod conflicts for instant results without scanning.

- [ ] **Native app integration** — Embedding the Rust library in a standalone web or desktop application.
