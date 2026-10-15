# Correctness and LoadFolders Sprint Design

## Intent

Make existing detector results more trustworthy by fixing known false negatives, parser data loss, and misleadingly incomplete output. Add active-mod-aware `LoadFolders.xml` resolution so the scanner selects the same content folders RimWorld would load. This sprint is limited to correctness and folder resolution, not the larger deferred product integrations.

## User-approved decisions

- Scope: correctness fixes plus `LoadFolders.xml`; the current checkout's unrelated fixture deletions and untracked plans remain untouched.
- Support RimWorld 1.6 `IfModActiveAll`, in addition to `IfModActive` and `IfModNotActive`.
- Active set: `ModsConfig.xml` `activeMods` for `--config`; all discovered mods for `--dir`; all explicitly supplied mods for `--mods`; all `mod_paths` for FFI.
- Match condition identifiers the way the game does: case-insensitive package ID or display name, with `_steam` package suffix tolerance.
- Use game-like file precedence: for duplicate relative paths, analyze only the highest-priority file across Defs, Patches, and Assemblies. Also correct implicit fallback so root folders remain available alongside version/Common folders.
- For malformed/unselectable `LoadFolders.xml`, emit a warning and fall back to implicit folder resolution. For conditions that select a valid version block but filter out every folder, retain the empty selection and warn; do not silently replace a deliberate empty result with implicit paths.
- Normalize revision-suffixed game versions to `X.Y` for folder selection.
- Include aggregated Info-level conflicts for non-overlapping XPath patches targeting the same concrete def.
- Change `list-mods --format json` to a stable `{ "mods": [...], "diagnostics": [...] }` envelope.
- Typed FFI config should warn and continue for unknown keys. Invalid field types return JSON errors; omitted fields retain existing defaults.
- Logical commits are authorized after tests/review. No push or merge to `main` is authorized.

## Goals and acceptance criteria

### XML conflict and parsing correctness

1. `XPathOnReplacedDef` emits one conflict per patch operation and distinct replacing mod; no early break suppresses additional replacers.
2. Reconstructed def XML preserves all root attributes. Attribute order is semantically irrelevant: normalize attributes deterministically. Attribute-only differences are included in `divergent_fields` as `@AttributeName`, so conflicts do not show an unexplained empty difference list.
3. XPath and list-item text parsing consumes through the matching end event, including empty, commented, CDATA, and ordinary text content. It does not consume a following sibling. Undecodable XML text produces diagnostics rather than disappearing or switching silently to a different normalization.
4. Same-def non-overlapping XPath operations produce one Info conflict per concrete `(def_type, def_name, mod pair)`, aggregating the distinct non-overlapping XPath pairs. Only pairs where both operations target the same explicit def are included; wildcard and different-def pairs are excluded. A new public conflict mechanism is required, and report `schema_version` advances to 2.
5. XML text inputs (About.xml, ModsConfig.xml, LoadFolders.xml, Defs XML, and Patches XML) have a 50 MiB per-file ceiling. Over-limit files are skipped whole and produce the existing context-appropriate diagnostic; do not partially parse them. DLL reads are not covered by this limit.
6. Recursive patch-operation parsing has a 64-level nesting ceiling. On overflow, warn and skip the over-deep operation subtree while preserving parseable siblings; no stack overflow or silent clean result.
7. `PatchOperationSequence.success` remains a conservative static-analysis limitation: assume contained operations may execute, and document that the runtime success attribute is not evaluated.

### FFI and CLI correctness

- Share the `ludeon.*` base-content classification between CLI and FFI resolution.
- Deserialize the FFI JSON through a typed config. Missing `mod_paths`, `game_version`, and `ordered` retain current defaults; invalid types return `error_json`. Preserve unknown fields for forward compatibility and add a configuration Warning to report diagnostics rather than silently ignoring misspellings.
- Document that the FFI progress callback's `phase` pointer is valid only during the synchronous callback invocation.
- `list-mods --format json` always emits the envelope, including empty results and resolution diagnostics. Keep human output and the command's exit-code ladder unchanged.

### LoadFolders.xml and folder resolution

- Detect the root `LoadFolders.xml` filename and `<loadFolders>` root case-insensitively. If absent, retain implicit folder selection, corrected so root content is included at lower priority after version-specific and Common folders. RimWorld 1.0 keeps implicit resolution.
- Normalize inputs such as `1.5.4085 rev442` or `1.7.4976` to `1.5` or `1.7` using strict numeric version parsing that preserves the existing path-traversal guard. Select exact `<vX.Y>` first, otherwise the greatest valid version `<= X.Y`, then `<default>`, then implicit fallback. With no valid requested version, use `<default>` or implicit fallback.
- In a selected version entry, interpret each child element as a folder entry (not only elements named `li`), preserving its text path. Empty text, `/`, and `\` map to the mod root; normalize path separators. Ignore comments. Folder path names remain case-sensitive. Reject absolute/traversal paths (except the root markers), emit a warning, and skip the invalid entry.
- `IfModActive` passes when any listed identity is active; `IfModActiveAll` passes when all listed identities are active; `IfModNotActive` passes when none of its listed identities are active. Comma-separated values are trimmed. Multiple recognized condition attributes on one entry are ANDed. A valid, unknown condition attribute is ignored as RimWorld does and reported once as Info per mod; do not reinterpret `IfModActiveAny` as a supported condition.
- A malformed/over-limit file or no applicable version entry emits a Warning and uses implicit folder resolution. A selected version block with all entries filtered out resolves to no folders, stays represented in `mods_scanned`, and emits a Warning explaining the empty selection. Missing listed directories are skipped without warning.
- Folder resolution must be a second pass once the active mod set is known. For `--config`, use the config list; for the other modes, use the approved all-input set. FFI uses `mod_paths` as its active set. Keep `Mod`'s public path fields as directory lists; do not add per-file vectors to the serialized/public shape.
- Return folders in priority order. For `LoadFolders.xml`, later listed folders have higher priority. For implicit selection, version-specific folders outrank Common, which outranks root. At each consumer, de-duplicate files by case-sensitive path relative to its content root, keeping the first/highest-priority file. Apply the same rule to Defs, Patches, and Assemblies.
- Folder diagnostics use the existing `FolderResolution` source; do not add a new `DiagnosticSource` variant for this feature.

## Scope boundaries

Included correctness backlog: XPath-on-replaced-def false negatives; root def attributes; fragile XPath/list text reads; replacement-text decode failures; shared base-content classification; typed FFI config and callback lifetime contract; machine-readable list-mods diagnostics; bounded XML and recursive patch parsing; and the spec-required same-def non-overlap Info result.

Excluded: C# in-game mod, RimSort integration, standalone application, conflict database/rule generator, field-level XML diffing, def inheritance or ModExtension conflict detection, new PatchOperation classes, generic error-type cleanup, About.xml parser consolidation, public-module visibility cleanup, and byte-for-byte comment/PI reconstruction. A test-only panic seam for the already-reported manual Harmony scan panic is excluded absent a stable fixture.

## Implementation constraints and verification

- Keep the existing Rust workspace and `quick-xml`; add no dependency unless the chosen shared file-enumeration design demonstrates it is needed.
- Use existing diagnostic plumbing; failures must be visible and must not become a clean, empty result.
- Add unit tests for parser, folder selection, conditions, precedence, and version normalization; add CLI/FFI tests for the output/config contracts and at least one end-to-end detector test.
- Run `just ci` (clippy with warnings denied, fmt, workspace tests, workspace build) on the integrated branch.
- Run the available local mod corpus at `/mnt/bulk/anten/.local/share/Steam/steamapps/workshop/content/294100` after the changes and capture before/after conflict counts plus new diagnostics. The corpus resolves 1,566 mods; LoadFolders filenames must be discovered case-insensitively rather than counted using one filename spelling. Treat this as a required false-positive review gate; if environment access prevents it, report it explicitly as unverified.
- Baseline at `d3dbd89`: valid JSON for 1,566 scanned mods, 88,355 conflicts (45 Error, 831 Warning, 87,479 Info): 29 direct replacements, 88,260 XPath overlaps, 2 XPath-on-replaced-def, 64 Harmony. It emitted 16,320 Info diagnostics (87 XML parsing, 16,233 DLL parsing), with no Warning/Error diagnostics. The command returned exit 1 because of the 45 Error conflicts; this was expected, not a failed scan. Baseline JSON is retained at `/tmp/rcd-corpus-before.json` (80,757,107 bytes).

## Evidence

- `TODO.md` and the final-review follow-ups define the correctness backlog.
- RimWorld Wiki, [Mod Folder Structure](https://rimworldwiki.com/wiki/Modding_Tutorials/Mod_Folder_Structure), documents `LoadFolders.xml` version folders, conditional attributes, and duplicate-path override behavior. The local 1.7.4976 installation and workshop corpus were also inspected by a read-only scout; disassembly-derived details (notably folder ordering and identifier matching) are treated as evidence to verify in tests rather than as an undocumented dependency.
- Integration points are `crates/rcd-core/src/folder.rs`, `crates/rcd-cli/src/resolve.rs`, `crates/rcd-ffi/src/lib.rs`, `crates/rcd-xml/src/lib.rs`, and `crates/rcd-harmony/src/lib.rs`.
