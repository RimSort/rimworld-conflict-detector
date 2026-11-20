# Changelog

## Unreleased

### Breaking

- Reports use schema version 3, with Rust-derived conflict severity and diagnostic completeness; the `xpath_same_def_non_overlap` mechanism carries aggregated XPath pairs.
- `list-mods --format json` returns `{ "mods": [...], "diagnostics": [...] }` instead of a bare array, including empty and partially resolved results.
- FFI configuration rejects invalid types for known fields instead of silently using defaults. Unknown fields produce a warning and analysis continues; omitted fields retain their defaults.

### Added

- Add optional, defaultable diagnostic `code` and named string `params` fields
  for metadata, load-order and folder resolution, XML reads, structural parsing,
  replacement reconstruction and skipped-def counts, DLL scanning, FFI
  configuration, and CLI input resolution. Consumers can use stable templates
  for known codes; absent or unknown codes fall back to the raw message, and
  embedded error details remain raw.
- Active-mod-aware `LoadFolders.xml` resolution with `IfModActive`, `IfModNotActive`, and `IfModActiveAll`, version/default selection, and visible fallback diagnostics.
- Deterministic file overlays across Defs, Patches, and Assemblies; later explicitly listed folders override earlier folders by relative file path.
- One Info finding per concrete def and mod pair for non-overlapping XPath patches, retaining unique XPath pairs.
- A 64-level recursive patch-operation limit, with diagnostics for skipped content.

### Fixed

- Follow a unique LoadFolders selector link to a regular file; warn as incomplete and fall back on unusable or competing selector names.
- Reject About metadata with elements left open at end of input through the existing parse-failure diagnostic.
- Preserve distinct replacement reconstruction diagnostics by their full represented message, code and params while coalescing exact repeats.
- Reject duplicate parsed package IDs in FFI requests, preserving metadata diagnostics; warn when invalid game versions require fallback.
- Surface manual DLL read failures as incomplete diagnostics; XML and DLL reads remain uncapped.
- Include implicit root content alongside version and Common folders; normalize revision-suffixed game versions before directory selection.
- Resolve the complete active set before conditional folder selection in CLI and FFI input modes; share official base-content classification.
- Report every distinct replacing mod when XPath patches target replaced defs, excluding self-conflicts.
- Preserve def-root attributes, canonicalize attribute order and equivalent entity spellings, and explain root-attribute differences in replacement findings.
- Consume empty, commented, and CDATA XPath/list text without losing following operations; expose decoding failures rather than silently discarding text.
- Propagate LoadFolders directory-inspection errors and validate XML structure explicitly.
- Ignore self-closing markup nested inside a `LoadFolders.xml` folder entry so it cannot inject an extra root-folder entry.
- Name every rejected `LoadFolders.xml` folder path in the selection warning, and keep the empty-selection status alongside those details.
- Keep `--config` folder diagnostics scoped to the mods the report actually scanned, without dropping unresolved active-mod warnings.
- Report differing def-root attributes in a fixed order so repeated scans of the same corpus list the same `divergent_fields`.
- Trim whitespace padded `CDATA` XPath text so the operation is normalized and analyzed like plain-text XPath.

### Documentation

- Clarify that the FFI progress callback's phase pointer is valid only during the synchronous callback.
- Document conservative static treatment of `PatchOperationSequence.success` rather than simulating runtime outcomes.
- Document uncapped input/report memory risks, the 64-level patch-operation limit, and implicit fallback across version, `Common/`, and the mod root.
