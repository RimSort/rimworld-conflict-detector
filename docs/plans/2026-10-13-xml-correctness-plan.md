# XML and Harmony Input Correctness Implementation Plan

> **For agentic workers:** Use the approved design at `docs/plans/2026-10-13-correctness-loadfolders-design.md`. This plan consumes the `read_xml_file` and `collect_files_with_precedence` interfaces from `2026-10-13-loadfolders-core-plan.md`.

**Goal:** Prevent silent loss or analysis of overridden XML/DLL content, preserve def identity, and bound XML/recursive patch parsing.

**Architecture:** Keep the existing parser return contracts, but add multi-directory entry points that use the core first-wins file list. Thread parse/decoding errors through existing diagnostics; do not silently drop malformed values.

**Tech Stack:** Rust 2024, `quick-xml`, existing core diagnostics and file-resolution helpers.

**Spec:** `docs/plans/2026-10-13-correctness-loadfolders-design.md`

## Global Constraints

- Read XML through `rcd_core::xml_io::read_xml_file`; skip over-limit files whole and report an error string to the existing diagnostic pipeline.
- Use `rcd_core::content_files::collect_files_with_precedence` for Defs, Patches, and Assemblies; do not independently reimplement relative-path precedence.
- Preserve partial valid input results where current parser contracts allow it; any skipped/untrusted content must be accompanied by diagnostics.
- Maximum recursive `PatchOperation` nesting is 64; exceeding it skips only that operation subtree and preserves siblings.

## Review Focus

- Empty/CDATA/commented XPath does not consume the next sibling operation.
- Empty/CDATA/commented list elements do not erase FindMod conditions or consume the following element.
- Root attributes are preserved and attribute order does not create replacement conflicts.
- Defs, patches, and assemblies duplicated at the same relative path are analyzed only from the priority winner.
- A file above 50 MiB or a patch operation deeper than 64 cannot produce a silent clean scan or stack overflow.

---

### Task 1: Def identity and replacement text correctness

**Files:** Modify `crates/rcd-xml/src/def_parser.rs` and `crates/rcd-xml/src/replacement.rs`.

**Interfaces:** Keep `parse_defs_dir` for compatibility; add `parse_defs_dirs(dirs: &[PathBuf]) -> (Vec<ParsedDef>, usize, usize, Vec<String>)` and make the one-directory function delegate to it. `replacement::detect_replacements` continues returning `(Vec<Conflict>, Vec<String>)`.

- [ ] Add a failing parser test showing root `Abstract`, `ParentName`, `Name`, and arbitrary attributes survive in `raw_xml` and remain well-formed.
- [ ] Add a failing replacement test showing attribute-only differences create a conflict and report divergent fields as `@AttributeName`.
- [ ] Add a test showing equivalent root attributes in different source order normalize identically.
- [ ] Add failing tests for undecodable text in reconstructed XML, proving the decoder emits a diagnostic and does not silently collapse different defs to an identical normalized string.
- [ ] Implement root attribute capture/threading; canonicalize attribute ordering deterministically in reconstructed elements; extend divergent-field extraction to compare root attributes while retaining child-field reporting.
- [ ] Thread `&mut Vec<String>` through normalization/text-extraction helpers that currently discard `unescape()` errors or silently choose a fallback canonical form. Keep valid-XML behavior unchanged and return errors through `detect_replacements`.
- [ ] Use the core precedence helper and bounded XML reader for multi-directory Defs parsing; preserve parse statistics and diagnostic behavior.
- [ ] Verify focused tests and existing replacement/def-parser tests pass.

### Task 2: Robust patch-operation text reads and nesting bound

**Files:** Modify `crates/rcd-xml/src/patch_ops.rs`.

**Interfaces:** Keep `parse_patches_dir`; add `parse_patches_dirs(dirs: &[PathBuf]) -> (Vec<ParsedPatchOp>, Vec<String>)`, with the existing API delegating to it. All recursive parsing helpers continue receiving the shared `errors` accumulator.

- [ ] Add failing tests for empty, comment-before-text, CDATA, and malformed `<xpath>` content; assert the following sibling operation survives and malformed input has a diagnostic.
- [ ] Add failing tests for empty/CDATA/commented `<li>` values in FindMod and Conditional operations; assert condition values and following operations are preserved.
- [ ] Add a test with a 65-level nested sequence followed by a valid sibling; assert a diagnostic, no panic, and preservation of the sibling.
- [ ] Replace one-shot Text-then-discard-End reads with event loops that consume the matching End, accept Text/CDATA, diagnose nested/mismatched/EOF/decode failures once, and resynchronize at the outer operation.
- [ ] Thread a depth counter through recursive operation parsing; on depth >64, warn and iteratively skip only the over-deep operation subtree.
- [ ] Use the bounded reader and core precedence helper for multi-directory parsing; over-limit files are skipped whole with the existing XML diagnostic path.
- [ ] Add a code-level explanation that `PatchOperationSequence.success` is intentionally not evaluated; conservatively assume all contained operations may execute.
- [ ] Verify focused tests and existing patch parsing tests pass.

### Task 3: XML detector and Harmony file overlays

**Files:** Modify `crates/rcd-xml/src/lib.rs`, `crates/rcd-harmony/src/lib.rs`, and `crates/rcd-harmony/src/discovery.rs`; consume the core file-precedence API from the LoadFolders plan.

**Interfaces:** Add a multi-directory Harmony discovery function (e.g. `discover_assemblies_dirs`) that returns the same `(Vec<PathBuf>, Vec<String>)` contract as the current one-directory function; keep the existing API as a wrapper.

- [ ] Add temp-directory integration tests with same-relative-path Defs/Patches/Assemblies in two folders: only the first/highest-priority file contributes, while unique relative files from both folders remain visible.
- [ ] Change `XmlDetector` to call the new multi-directory parser functions once per mod, avoiding separate per-folder parses that lose the shared dedup set.
- [ ] Change `HarmonyDetector` to recursively enumerate priority-winner files across `assemblies_paths`; retain per-file parse warnings and exclude non-DLL files as today.
- [ ] Confirm deterministic ordering by sorting each directory's relative file list before first-wins selection.
- [ ] Verify focused integration tests and all XML/Harmony crate tests pass.
