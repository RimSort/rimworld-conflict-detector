# Changelog

## Unreleased

### Breaking

- Reports use schema version 2, adding the `xpath_same_def_non_overlap` mechanism with aggregated XPath pairs. Existing mechanism tags are unchanged.
- `list-mods --format json` returns `{ "mods": [...], "diagnostics": [...] }` instead of a bare array, including empty and partially resolved results.
- FFI configuration rejects invalid types for known fields instead of silently using defaults. Unknown fields produce a warning and analysis continues; omitted fields retain their defaults.

### Added

- Active-mod-aware `LoadFolders.xml` resolution with `IfModActive`, `IfModNotActive`, and `IfModActiveAll`, version/default selection, and visible fallback diagnostics.
- Deterministic file overlays across Defs, Patches, and Assemblies; later explicitly listed folders override earlier folders by relative file path.
- One Info finding per concrete def and mod pair for non-overlapping XPath patches, retaining unique XPath pairs.
- A 50 MiB per-file limit for XML inputs and a 64-level recursive patch-operation limit, with diagnostics for skipped content.

### Fixed

- Include implicit root content alongside version and Common folders; normalize revision-suffixed game versions before directory selection.
- Resolve the complete active set before conditional folder selection in CLI and FFI input modes; share official base-content classification.
- Report every distinct replacing mod when XPath patches target replaced defs, excluding self-conflicts.
- Preserve def-root attributes, canonicalize attribute order and equivalent entity spellings, and explain root-attribute differences in replacement findings.
- Consume empty, commented, and CDATA XPath/list text without losing following operations; expose decoding failures rather than silently discarding text.
- Propagate LoadFolders directory-inspection errors and validate XML structure explicitly.

### Documentation

- Clarify that the FFI progress callback's phase pointer is valid only during the synchronous callback.
- Document conservative static treatment of `PatchOperationSequence.success` rather than simulating runtime outcomes.
