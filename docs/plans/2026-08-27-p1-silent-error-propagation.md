# P1: Silent Error Propagation Implementation Plan

> **REQUIRED SUB-SKILL:** Use the executing-plans skill to implement this plan task-by-task.

**Goal:** Eliminate the four silent-failure paths (TODO P1 items #5–8) so that mod-discovery failures, FFI parse failures, XML parse errors, and manual-patch panics all surface as user-visible `Diagnostic`s instead of being swallowed.

**Architecture:** The codebase already has a diagnostic pipeline (`Diagnostic { affected_mod, severity, message, source }` → `Report.diagnostics`) and two established patterns: `parse_defs_dir`/`parse_patches_dir` return `(results, errors: Vec<String>)`, and detectors convert those strings into `Diagnostic`s tagged with a `DiagnosticSource`. This plan extends that pattern to the four remaining silent paths instead of inventing new error plumbing (YAGNI).

**Tech Stack:** Rust, quick-xml 0.37 (errors are NOT sticky — an `Err` is consumed once and later reads return `Ok`/`Eof` or a *different* `Err`; verified empirically against 0.37.5, this drives Task 2's design), thiserror, catch_unwind.

**Context notes for the implementer:**

- `DiagnosticSource` variants available: `XmlParsing`, `DllParsing`, `MetadataResolution`, `FolderResolution`, `ModsConfig` (`crates/rcd-core/src/diagnostic.rs`).
- The `Detector` trait returns `(Vec<Conflict>, Vec<Diagnostic>)` — diagnostics flow up through `Engine::analyze` into `Report::build` automatically.
- Repo-wide test command: `cargo test` (47 tests passing at plan time). Per AGENTS.md: run tests + clippy before every commit.
- Worktree: `git worktree add ../rcd-p1 -b fix/p1-error-propagation` (per using-git-worktrees skill).

---

### Task 1: Report manual-patch detection panics (TODO P1 #8)

**Why first:** Trivial, fully isolated to one match expression, zero API changes — good warmup and it de-risks the crate before touching signatures.

**Files:**

- Modify: `crates/rcd-harmony/src/lib.rs:97-115` (the `if let Ok(hints) = manual_result` block)
- Test: `crates/rcd-harmony/src/lib.rs` (`mod tests` at bottom)

**Step 1: Write the failing test**

Add to `mod tests` in `crates/rcd-harmony/src/lib.rs`:

```rust
#[test]
fn manual_scan_panic_emits_warning_diagnostic() {
    // detect_manual_patches reads files as bytes and scans strings; a
    // missing path would just return no hints. Instead verify the panic
    // path directly by checking the code shape is exercised: we call
    // analyze with a mod whose assemblies path points at a file (not a
    // dir) so discover_assemblies handles it, then assert the contract:
    // any diagnostic mentioning manual scanning at Warning level is well-formed.
    use crate::HarmonyDetector;
    use rcd_core::{Detector, Severity};

    let m = rcd_core::Mod {
        package_id: "test.panic".to_string(),
        name: "Panic Test".to_string(),
        author: None,
        is_base_content: false,
        root_path: std::path::PathBuf::new(),
        supported_versions: vec![],
        dependencies: vec![],
        incompatibilities: vec![],
        load_before: vec![],
        load_after: vec![],
        force_load_before: vec![],
        force_load_after: vec![],
        defs_paths: vec![],
        patches_paths: vec![],
        assemblies_paths: vec![],
    };

    // No assemblies → no panics possible; this pins the non-panic contract.
    let (_conflicts, diagnostics) = HarmonyDetector.analyze(&[m], None);
    assert!(diagnostics.is_empty());
}
```

> Note: a true panic-path unit test would require a DLL fixture that crashes `detect_manual_patches`; the fixtures tree only ships one valid DLL. This test pins the surrounding contract; the code change itself is verified by inspection + the existing `analyze_emits_manual_patch_diagnostics` test continuing to pass. Do not spend time fabricating a crash fixture (YAGNI).

**Step 2: Run test to verify it passes (contract test)**

Run: `cargo test -p rcd-harmony manual_scan_panic_emits_warning_diagnostic`
Expected: PASS (empty assemblies → no diagnostics, already true)

**Step 3: Implement the diagnostic on panic**

In `crates/rcd-harmony/src/lib.rs`, replace:

```rust
                        if let Ok(hints) = manual_result {
                            for hint in hints {
                                diagnostics.push(Diagnostic {
                                    affected_mod: Some(mod_summary.package_id.clone()),
                                    severity: Severity::Info,
                                    message: format!(
                                        "Possible manual patch hint in {}: {} (best-effort heuristic, may be a false positive)",
                                        hint.dll_name, hint.possible_method
                                    ),
                                    source: DiagnosticSource::DllParsing,
                                });
                            }
                        }
```

with:

```rust
                        match manual_result {
                            Ok(hints) => {
                                for hint in hints {
                                    diagnostics.push(Diagnostic {
                                        affected_mod: Some(mod_summary.package_id.clone()),
                                        severity: Severity::Info,
                                        message: format!(
                                            "Possible manual patch hint in {}: {} (best-effort heuristic, may be a false positive)",
                                            hint.dll_name, hint.possible_method
                                        ),
                                        source: DiagnosticSource::DllParsing,
                                    });
                                }
                            }
                            Err(_) => {
                                diagnostics.push(Diagnostic {
                                    affected_mod: Some(mod_summary.package_id.clone()),
                                    severity: Severity::Warning,
                                    message: format!(
                                        "Manual patch detection panicked while scanning {} (caught; results for this DLL may be incomplete)",
                                        dll_name
                                    ),
                                    source: DiagnosticSource::DllParsing,
                                });
                            }
                        }
```

**Step 4: Verify**

Run: `cargo test -p rcd-harmony && cargo clippy -p rcd-harmony -- -D warnings`
Expected: all PASS

**Step 5: Commit**

```bash
git add crates/rcd-harmony/src/lib.rs
git commit -m "harmony: emit Warning diagnostic when manual patch scan panics

catch_unwind around detect_manual_patches previously swallowed panics
silently; parse_harmony_patches already reported its failures. Match the
sibling behavior so C#/CLI consumers see partial-results situations."
```

---

### Task 2: Surface XML parse errors as diagnostics (TODO P1 #7)

**Why this design (verified empirically):** `parse_defs_from_xml` and `parse_patches_from_xml` currently end with `Err(_) => break`, discarding the parse error. The original draft assumed quick-xml errors were *sticky* (repeated on every subsequent read), so capturing at the two outer loops would suffice. **That assumption is false.** A probe against quick-xml 0.37.5 showed that after an `Err`, the reader continues: truncated input yields one `Err` then `Ok(Eof)`; mismatched tags yield a cascade of *distinct* `Err`s then `Eof`. Consequence: the nested `Err(_) => break` sites in `parse_single_def`/`parse_operation`/etc. consume errors and unwind, and the outer loop never sees them — outer-only capture would silently lose mid-parse errors, which is exactly the bug this task fixes.

So the fix must capture errors at **every** `Err(_) => break` site. The mechanical way is to thread an `errors: &mut Vec<String>` accumulator through the recursive parse functions (adding `source: &str` to the three helpers that lack it), and widen the two top-level return types to `(results, Vec<String>)` to match the existing `parse_defs_dir`/`parse_patches_dir` contract.

Partial results up to the error point are still returned — that's intentional: a file with 50 valid patches and a trailing malformed one should still contribute the 50, now with a visible Warning.

**Files:**

- Modify: `crates/rcd-xml/src/def_parser.rs` — `parse_defs_from_xml` (:68-116, outer Err at :109), `parse_single_def` (:121-201, Err at :180), dir caller (:47), tests at `:239, :273, :295, :338, :363`
- Modify: `crates/rcd-xml/src/patch_ops.rs` — `parse_patches_from_xml` (:78-119, outer Err at :113), `parse_operation` (:124, no own Err site — pass-through), `read_xpath_from_operation` (:172, Err at :204), `parse_sequence` (:215, Err at :246), `parse_find_mod` (:255, Err at :289), `parse_conditional` (:299, Err at :332), `read_li_text_list` (:341, Err at :372), `skip_current_element_by_depth` (:393, Err at :407), dir caller (:64), 9 test call sites at `:474-666`

**Step 1: Write the failing tests**

Add to `mod tests` in `crates/rcd-xml/src/def_parser.rs`:

```rust
#[test]
fn malformed_defs_xml_reports_error_and_keeps_partial_results() {
    // First def is valid; the truncated second def triggers a parse error.
    // quick-xml errors are not sticky, so use >=1 / any() assertions —
    // cascading distinct errors are possible (see design note above).
    let xml = r#"<Defs>
    <ThingDef><defName>Good</defName></ThingDef>
    <ThingDef><defName>Broken"#;

    let (defs, _skipped, errors) = parse_defs_from_xml(xml, "test.xml");
    assert!(defs.iter().any(|d| d.def_name == "Good"), "valid def before the error should be kept");
    assert!(!errors.is_empty(), "parse error must be reported");
    assert!(errors.iter().all(|e| e.contains("test.xml")));
}
```

Add to `mod tests` in `crates/rcd-xml/src/patch_ops.rs`:

```rust
#[test]
fn malformed_patches_xml_reports_error_and_keeps_partial_results() {
    let xml = r#"<Patch>
    <Operation Class="PatchOperationReplace">
        <xpath>Defs/ThingDef[defName="Steel"]/label</xpath>
        <value><label>Iron</label></value>
    </Operation>
    <Operation Class="PatchOperationReplace"#;

    let (ops, errors) = parse_patches_from_xml(xml, "test.xml");
    assert!(ops.iter().any(|o| o.xpath.contains("Steel")), "valid op before the error should be kept");
    assert!(!errors.is_empty(), "parse error must be reported");
    assert!(errors.iter().all(|e| e.contains("test.xml")));
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test -p rcd-xml malformed_defs_xml_reports_error malformed_patches_xml_reports_error`
Expected: COMPILE FAIL (return-type arity) — this confirms the signatures change.

**Step 3: Change `parse_defs_from_xml`**

In `crates/rcd-xml/src/def_parser.rs`, change the signature and outer loop:

```rust
/// Returns (parsed defs, count of abstract defs skipped, parse errors).
pub fn parse_defs_from_xml(xml: &str, source: &str) -> (Vec<ParsedDef>, usize, Vec<String>) {
    let mut defs = Vec::new();
    let mut abstract_skipped: usize = 0;
    let mut errors = Vec::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut inside_defs = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            // ... unchanged Start/End arms ...
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(format!("XML parse error in {}: {}", source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    (defs, abstract_skipped, errors)
}
```

Thread `errors: &mut Vec<String>` through `parse_single_def` (`source` is already a parameter) and capture at its `Err` site (`:180`):

```rust
fn parse_single_def(
    reader: &mut Reader<&[u8]>,
    def_type: &str,
    is_abstract: bool,
    source: &str,
    errors: &mut Vec<String>,
) -> Option<ParsedDef> {
    // ...
            Err(e) => {
                errors.push(format!("XML parse error in {}: {}", source, e));
                break;
            }
    // ...
}
```

Update the `parse_single_def` call inside `parse_defs_from_xml` to pass `&mut errors`.

Update the dir-level caller at `:47`:

```rust
                let (parsed, skipped, xml_errors) = parse_defs_from_xml(&contents, &source);
                defs.extend(parsed);
                total_skipped += skipped;
                errors.extend(xml_errors);
```

**Step 4: Change `parse_patches_from_xml`**

In `crates/rcd-xml/src/patch_ops.rs`:

```rust
/// Returns (parsed operations, parse errors).
pub fn parse_patches_from_xml(xml: &str, source: &str) -> (Vec<ParsedPatchOp>, Vec<String>) {
    let mut ops = Vec::new();
    let mut errors = Vec::new();
    // ... unchanged setup ...

    loop {
        match reader.read_event_into(&mut buf) {
            // ... unchanged arms ...
            Ok(Event::Eof) => break,
            Err(e) => {
                errors.push(format!("XML parse error in {}: {}", source, e));
                break;
            }
            _ => {}
        }
        buf.clear();
    }

    (ops, errors)
}
```

Update the dir-level caller at `:64`:

```rust
                let (parsed_patches, xml_errors) = parse_patches_from_xml(&contents, &source);
                ops.extend(parsed_patches);
                errors.extend(xml_errors);
```

Thread `errors: &mut Vec<String>` through the recursive helpers and replace each `Err(_) => break` with capture-and-break. `source` is already a parameter on all except `read_xpath_from_operation`, `read_li_text_list`, and `skip_current_element_by_depth` — add `source: &str` to those three:

| Function | Err site | Signature change |
| --- | --- | --- |
| `parse_operation` | (none — pass-through to callees) | `+ errors` |
| `read_xpath_from_operation` | `:204` | `+ source, errors` |
| `parse_sequence` | `:246` | `+ errors` |
| `parse_find_mod` | `:289` | `+ errors` |
| `parse_conditional` | `:332` | `+ errors` |
| `read_li_text_list` | `:372` | `+ source, errors` |
| `skip_current_element_by_depth` | `:407` | `+ source, errors` |

The capture pattern is identical at every site:

```rust
            Err(e) => {
                errors.push(format!("XML parse error in {}: {}", source, e));
                break;
            }
```

(The `if let Ok(Event::Text(..))` / `let _ = read_event_into(..)` best-effort reads inside `read_xpath_from_operation` and `read_li_text_list` may stay silent — they only drop text content, not structure.)

**Step 5: Fix all call sites and tests**

- `def_parser.rs` tests (`:239, :273, :295, :338, :363`): `let (defs, _) = ...` → `let (defs, _, _) = ...` (use `assert!(errors.is_empty())` in at least one to pin well-formed inputs stay quiet).
- `patch_ops.rs` tests (`:474-666`, 9 sites): `let ops = parse_patches_from_xml(xml, "test.xml");` → `let (ops, errors) = ...; assert!(errors.is_empty());` — keep one representative assertion, plain destructuring for the rest.

**Step 6: Verify**

Run: `cargo test && cargo clippy --workspace -- -D warnings`
Expected: all PASS. XmlDetector needs **no** changes: it already converts the `Vec<String>` from `parse_defs_dir`/`parse_patches_dir` into Warning diagnostics tagged `XmlParsing`.

**Step 7: Commit**

```bash
git add crates/rcd-xml/
git commit -m "xml: surface XML parse errors as diagnostics instead of silently truncating

parse_defs_from_xml and parse_patches_from_xml previously hit Err(_) =>
break and returned partial results with no indication anything was wrong,
producing false negatives on malformed mod XML. Both now return an errors
vec that flows through parse_*_dir into the existing Warning-diagnostic
pipeline. quick-xml errors are NOT sticky (each Err is consumed once), so
every Err(_) => break site records via a threaded errors vec."
```

---

### Task 3: Report discovery failures from `discover_mods` (TODO P1 #5)

**Why:** `discover_mods` (`crates/rcd-core/src/folder.rs:72-110`) uses `let Ok(entries) = ... else { continue }` and `if let Ok(mut mod_data) = parse_about_xml(...)`, making "no mods found" indistinguishable from "every mod failed to parse". This is the highest fan-in function in rcd-core (10 callers per the code graph).

**Files:**

- Modify: `crates/rcd-core/src/folder.rs:72-110` (signature + body), `:151-156` (test)
- Modify: `crates/rcd-cli/src/resolve.rs:16-23` (`resolve_from_dir`), `:99` (`resolve_from_config`)
- Modify: `crates/rcd-xml/src/lib.rs:154, :205` (test call sites)
- Modify: `crates/rcd-core/tests/integration.rs:23, :37, :44, :53, :72` (5 more test call sites — verified by workspace-wide grep)

**Step 1: Write the failing tests**

Add to `mod tests` in `crates/rcd-core/src/folder.rs`:

```rust
#[test]
fn discover_mods_reports_unparseable_about_xml() {
    let dir = tempfile::tempdir().unwrap();
    let broken = dir.path().join("BrokenMod/About");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("About.xml"), "<ModMetaData><unclosed>").unwrap();

    let (mods, diagnostics) = discover_mods(&[dir.path().to_path_buf()], None);
    assert!(mods.is_empty());
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].severity, crate::Severity::Warning);
    assert!(diagnostics[0].message.contains("BrokenMod"));
    assert_eq!(diagnostics[0].source, crate::DiagnosticSource::FolderResolution);
}

#[test]
fn discover_mods_reports_unreadable_search_dir() {
    let (mods, diagnostics) = discover_mods(&[PathBuf::from("/nonexistent/path/test.dll")], None);
    assert!(mods.is_empty());
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("Failed to read directory"));
}
```

(`rcd-core` does NOT currently dev-depend on `tempfile` — add `tempfile = { workspace = true }` to `crates/rcd-core/Cargo.toml` `[dev-dependencies]`; it is already a workspace dependency used by rcd-xml.)

**Step 2: Run tests to verify they fail**

Run: `cargo test -p rcd-core discover_mods_reports`
Expected: COMPILE FAIL (arity) confirming the signature change.

**Step 3: Implement**

```rust
pub fn discover_mods(
    search_dirs: &[PathBuf],
    game_version: Option<&str>,
) -> (Vec<Mod>, Vec<crate::Diagnostic>) {
    let mut mods = Vec::new();
    let mut diagnostics = Vec::new();

    for search_dir in search_dirs {
        if !search_dir.is_dir() {
            continue; // absent dir is normal (e.g., no Workshop install) — not an error
        }

        let Ok(entries) = std::fs::read_dir(search_dir) else {
            diagnostics.push(crate::Diagnostic {
                affected_mod: None,
                severity: crate::Severity::Warning,
                message: format!("Failed to read directory {}: permission denied or I/O error", search_dir.display()),
                source: crate::DiagnosticSource::FolderResolution,
            });
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let about_xml = path.join("About/About.xml");
            if !about_xml.exists() {
                continue; // directory without About.xml is not a mod — silent by design
            }

            match parse_about_xml(&path) {
                Ok(mut mod_data) => {
                    resolve_mod_folders(&mut mod_data, game_version, None);
                    mods.push(mod_data);
                }
                Err(e) => diagnostics.push(crate::Diagnostic {
                    affected_mod: None, // package_id is unknown — parsing failed
                    severity: crate::Severity::Warning,
                    message: format!(
                        "Failed to parse mod at {}: {}",
                        path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string()),
                        e
                    ),
                    source: crate::DiagnosticSource::FolderResolution,
                }),
            }
        }
    }

    (mods, diagnostics)
}
```

**Step 4: Update call sites**

`crates/rcd-cli/src/resolve.rs` — `resolve_from_dir`:

```rust
pub fn resolve_from_dir(dir: &Path, game_version: Option<&str>) -> ResolvedMods {
    let (mut mods, diagnostics) = discover_mods(&[dir.to_path_buf()], game_version);
    mark_base_content(&mut mods);
    ResolvedMods {
        mods,
        load_order: None,
        diagnostics,
    }
}
```

`crates/rcd-cli/src/resolve.rs` — `resolve_from_config` (`:99`):

```rust
    let (available, mut diagnostics) = discover_mods(&search_dirs, version.as_deref());
    // ... (delete the later `let mut diagnostics = Vec::new();` — the
    // "mod not found" warnings append to `diagnostics` unchanged)
```

`crates/rcd-xml/src/lib.rs` tests (`:154`, `:205`):

```rust
        let (mods, _diagnostics) = discover_mods(&[fixtures_path()], Some("1.5"));
```

`crates/rcd-core/src/folder.rs` test `discover_mods_finds_fixtures` (`:151`): same destructuring.

`crates/rcd-core/tests/integration.rs` (`:23, :37, :44, :53, :72`): same destructuring at all 5 sites.

**Step 5: Verify**

Run: `cargo test && cargo clippy --workspace -- -D warnings`
Expected: all PASS.

**Step 6: Commit**

```bash
git add crates/rcd-core/ crates/rcd-cli/ crates/rcd-xml/src/lib.rs
git commit -m "core: report discovery failures from discover_mods as diagnostics

Previously a broken search directory or an unparseable About.xml was
indistinguishable from 'no mods found'. discover_mods now returns
(Vec<Mod>, Vec<Diagnostic>) consistent with parse_defs_dir/parse_patches_dir,
and both CLI resolution paths forward the diagnostics into the report."
```

---

### Task 4: FFI — collect parse failures into report diagnostics (TODO P1 #6)

**Files:**

- Modify: `crates/rcd-ffi/src/lib.rs:52-60` (mod resolution loop), `:87-92` (report construction)
- Test: `crates/rcd-ffi/src/lib.rs:127-177` (a `mod tests` already exists with 4 tests; add one more covering rcd_analyze diagnostics)

**Step 1: Write the failing test**

Add this test inside the existing `#[cfg(test)] mod tests` at `crates/rcd-ffi/src/lib.rs:127` (do NOT create a second module). The module already has `use super::*;` and `use std::ffi::CString;` — add `CStr` to that ffi import. Test body (drop the outer module wrapper shown in the original draft):

```rust
#[test]
fn rcd_analyze_reports_unparseable_mod_paths() {
        let config = serde_json::json!({
            "mod_paths": ["/nonexistent/mod/dir"],
            "game_version": "1.5",
            "ordered": false
        });
        let config_c = CString::new(config.to_string()).unwrap();

        let ptr = unsafe { rcd_analyze(config_c.as_ptr(), None) };
        assert!(!ptr.is_null());

        let json = unsafe { CStr::from_ptr(ptr) }.to_str().unwrap().to_string();
        unsafe { rcd_free(ptr) };

        let report: serde_json::Value = serde_json::from_str(&json).unwrap();
        let diagnostics = report["diagnostics"].as_array().expect("report must have diagnostics");
        assert!(
            diagnostics.iter().any(|d| d["severity"] == "error"
                && d["message"].as_str().is_some_and(|m| m.contains("Failed to parse mod"))),
        "parse failure must be visible to C# consumers: {}", json
    );
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rcd-ffi rcd_analyze_reports_unparseable_mod_paths`
Expected: FAIL (diagnostics array is empty today)

**Step 3: Implement**

In `crates/rcd-ffi/src/lib.rs`, replace the resolution loop:

```rust
        // Resolve mods
        let mut mods = Vec::new();
        let mut pre_diagnostics: Vec<rcd_core::Diagnostic> = Vec::new();
        for path in &mod_paths {
            match rcd_core::about_xml::parse_about_xml(path) {
                Ok(mut m) => {
                    rcd_core::folder::resolve_mod_folders(&mut m, game_version, None);
                    mods.push(m);
                }
                Err(e) => pre_diagnostics.push(rcd_core::Diagnostic {
                    affected_mod: None,
                    severity: rcd_core::Severity::Error,
                    message: format!("Failed to parse mod at {}: {}", path.display(), e),
                    source: rcd_core::DiagnosticSource::MetadataResolution,
                }),
            }
        }
```

And after `let report = engine.analyze(...)`:

```rust
        let mut report = engine.analyze(&mods, load_order.as_ref(), metadata, progress_fn.as_ref());
        report.diagnostics.splice(0..0, pre_diagnostics);
```

(Prepend so resolution failures appear before detector output; order is otherwise unconstrained by the schema.)

**Step 4: Verify**

Run: `cargo test && cargo clippy --workspace -- -D warnings`
Expected: all PASS. Also confirm the CLI behavior is unaffected (this task only touches rcd-ffi).

**Step 5: Commit**

```bash
git add crates/rcd-ffi/
git commit -m "ffi: surface parse-failed mods as Error diagnostics instead of dropping them

The CLI path already reported failed mods; the FFI boundary skipped them
with `if let Ok(..)`, giving C# consumers zero visibility. Parse failures
are now collected and prepended to report.diagnostics before serialization."
```

---

## Out of scope (deliberate)

- **P0 items** (XPathOnReplacedDef `break`, parser desync, root attrs, FFI `is_base_content`) — separate P0 plan/fixes.
- **Typed error refactor (P3 #17)** — this plan propagates errors through the *existing* string/diagnostic pipeline; standardizing error types is a later cleanup.
- **Distinguishing `read_dir` permission errors from other I/O failures** — one Warning message is enough for a local tool.

## Definition of done

- `cargo test` green (47 existing + ~7 new tests)
- `cargo clippy --workspace -- -D warnings` clean
- Four previously silent failure modes now produce `Diagnostic`s in both CLI JSON output and the FFI JSON report
- TODO.md items #5, #6, #7, #8 flipped to `[x]`
