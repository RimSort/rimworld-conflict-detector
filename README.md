# RimWorld Conflict Detector (rcd)

A static analysis tool that detects mod conflicts in [RimWorld](https://rimworldgame.com/). Analyzes mod definitions, XML patches, and Harmony patches to surface conflicts before they cause problems in-game.

## What It Detects

- **Direct Def Replacements** -- multiple mods defining the same `ThingDef`, `RecipeDef`, etc., where one silently overwrites another
- **XPath Overlap Conflicts** -- multiple mods patching overlapping XML paths
- **XPath on Replaced Defs** -- patches targeting defs that another mod has already replaced entirely
- **Harmony Patch Conflicts** -- multiple mods hooking the same C# method via Harmony, with priority analysis and short-circuit risk detection

All conflicts are load-order-aware, so the tool can tell you which mod "wins" and which gets overwritten.

## Quick Start

```bash
# Build
cargo build --release -p rcd-cli

# Scan a mods directory
rcd scan --dir ~/RimWorld/Mods

# Scan using your active mod list
rcd scan --config ~/.config/unity3d/Ludeon\ Studios/RimWorld/Config/ModsConfig.xml

# Check specific mods against each other
rcd scan --mods ./ModA ./ModB ./ModC
```

## Usage

```
rcd <COMMAND> [OPTIONS]

Commands:
  scan        Scan mods for conflicts
  list-mods   List discovered mods

Input (mutually exclusive):
  --dir <PATH>              Scan all mods in a directory
  --mods <PATH>...          Analyze specific mod directories
  --config <PATH>           Use RimWorld's ModsConfig.xml for active mod list & load order

Output:
  --format <human|json>     Output format (default: human)
  --severity <error|warning|info>  Minimum severity to display in human output (default: info; JSON always emits everything; the trailing `Summary:` diagnostic counts are always totals)
  --group-by <severity|mod> Grouping in human output (default: severity)

Other:
  --game-version <VER>      Target game version (e.g. "1.5")
  --rimworld-path <PATH>    RimWorld install directory (auto-detected on Steam)
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0    | Scan completed with no error-level conflicts and no error-level diagnostics |
| 1    | Error-level conflicts detected |
| 2    | No mods found AND resolution diagnostics present; also usage/fatal errors |
| 3    | Scan completed with error-level diagnostics (e.g. an explicitly-named mod path failed to parse, or a hard (forceLoad*) load-order violation) |

Precedence: 2 beats 1 beats 3. An empty scan with **no** diagnostics exits 0
(e.g. `--dir` pointing at an existing but empty directory). A typo'd or
nonexistent `--dir` is user-named input, so it yields an Error diagnostic and
exits 2; absent directories among `--config` search paths (Data/Mods/Workshop)
remain skipped by design, as not every install has all three.

The table describes `rcd scan`. `rcd list-mods` exits `2` whenever it lists no
mods — including an empty but valid input with no diagnostics.

### JSON Output

`rcd scan --format json` emits a report with `schema_version: 3`. Each conflict
includes its Rust-derived `severity` (`error`, `warning`, or `info`). Each
diagnostic has `analysis_incomplete`, which is true when analysis is known to
have been skipped or degraded. False does not establish exhaustive analysis.
Diagnostics can carry a nullable stable `code` and `params`, an object of named
string values. Either field may be omitted in v3; consumers should default a
missing code to null and missing params to an empty object. Consumers can render
known codes with stable templates, and should use `message` unchanged when the
code is absent or unknown. Structured coverage currently
includes metadata and load-order validation, folder selection and discovery, XML
file discovery and reads, skipped-def counts, structural parsing and replacement
reconstruction, Harmony and manual DLL scanning, FFI configuration, and CLI
input resolution. Parser- and system-provided error details remain raw strings
within params, even when a surrounding template is translated. Replacement
reconstruction diagnostics retain distinct represented message, code and params
combinations while coalescing exact repeats; they do not identify every source
occurrence. About metadata with unclosed elements at end of input is rejected
and reported as incomplete.
The aggregated non-overlapping same-def XPath Info conflict mechanism retains
its `xpath_same_def_non_overlap` tag and paired XPath expressions.

```json
{
  "type": "def",
  "severity": "error",
  "target": {"def_type": "ThingDef", "def_name": "Steel"},
  "mods": [],
  "mechanism": {"direct_replacement": {"divergent_fields": []}},
  "winner": null,
  "condition": null
}
```

`rcd list-mods --format json` always emits an object with both `mods` and
`diagnostics` arrays, including when discovery is partial or finds no mods:

```json
{
  "mods": [],
  "diagnostics": []
}
```

Resolution diagnostics are included in this envelope rather than printed as
non-JSON stderr text. The command retains its existing exit codes.

### Folder and Patch Analysis Limits

When a mod has `LoadFolders.xml`, folder selection uses the requested game
version (revision suffixes are normalized); for `--dir` and `--mods`, the
supplied mods form the active set, while `--config` uses the configured active
mods. For the requested version, selection prefers an exact entry, then the
greatest lower version entry; if neither applies, it uses `default` when present.
A malformed `LoadFolders.xml`, or the absence of both an applicable version
entry and a `default` entry, warns and falls back to implicit folder selection,
which searches the requested version folder, then `Common/`, then the mod root,
in that priority order. A selected entry that intentionally lists no content
folders stays empty. A unique case-insensitive `LoadFolders.xml` selector may
be a link to a regular file. Unusable selectors or competing selector names
produce an incomplete warning and use implicit fallback. This differs from
nested links within selected content directories, which are not followed and
are diagnosed. Game version 1.0 uses implicit folder resolution without
inspecting the selector.

XML inputs, managed DLL assemblies, and generated reports are not size-limited;
large inputs or many findings can consume substantial memory or exhaust it.
Patch operations nested more than 64 levels deep are skipped along with their
subtree while sibling operations are still analyzed, with a warning diagnostic.

Patch analysis is static: `PatchOperationSequence` operations are treated as
executing, and the sequence's `success` branch behavior is not evaluated.

## Building

Requires Rust 1.85+ (edition 2024).

```bash
# Build everything
just build

# Run tests
just test

# Clippy + fmt checks
just check

# Release build (CLI)
just package-cli

# Release build (FFI library for C# mod integration)
just package-mod
```

## Project Structure

```
crates/
  rcd-cli/      CLI binary
  rcd-core/     Core types, detector trait, report builder, mod resolution
  rcd-xml/      XML def & patch conflict detection
  rcd-harmony/  Harmony (.NET DLL) patch conflict detection
  rcd-ffi/      C-compatible FFI for embedding in a RimWorld mod
```

## How It Works

1. **Resolve mods** from input (directory scan, explicit paths, or ModsConfig.xml)
2. **Parse metadata** from each mod's `About.xml` (name, dependencies, load order)
3. **Parse defs and patches** from version-appropriate folders (`1.5/`, `Common/`, etc.)
4. **Detect conflicts** across all loaded mods using pluggable detectors
5. **Validate load order** against declared constraints
6. **Report** results as human-readable tables or structured JSON

## License

AGPL-3.0-or-later
