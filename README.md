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
  --severity <error|warning|info>  Minimum severity to display (default: info)
  --group-by <severity|mod> Grouping in human output (default: severity)

Other:
  --game-version <VER>      Target game version (e.g. "1.5")
  --rimworld-path <PATH>    RimWorld install directory (auto-detected on Steam)
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0    | No error-level conflicts found |
| 1    | Error-level conflicts detected |
| 2    | Fatal error (no mods found, invalid arguments) |

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
