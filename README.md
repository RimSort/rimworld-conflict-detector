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

`rcd scan --format json` emits a report with `schema_version: 2`. The version
covers the aggregated non-overlapping same-def XPath Info conflict mechanism,
whose serialized mechanism tag is `xpath_same_def_non_overlap` and whose
`xpath_pairs` contain the paired XPath expressions.

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
mods. If `LoadFolders.xml` cannot be parsed or no applicable version entry is
selectable, resolution warns and falls back to implicit versioned/Common folder
selection. A selected entry that intentionally lists no content folders stays
empty. Game version 1.0 uses implicit folder resolution.

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
