# P1 Final-Review Fixes Implementation Plan

> **REQUIRED SUB-SKILL:** Use the executing-plans skill to implement this plan task-by-task.

**Goal:** Close the 4 Important + 5 Minor findings from the final cross-commit review of `fix/p1-error-propagation`: JSON total-failure report (I-1), misleading "not found" message (I-2), unified About.xml-failure rendering (I-3), guaranteed malformed-XML visibility via well-formedness pre-check (I-4), plus replacement.rs capture, io-error detail, CLI/FFI ordering, end-to-end CLI tests, and the exit-code ladder.

**Architecture:** All decisions settled with the user: I-4 = full-document well-formedness pre-check (one choke point per parser, nested `if let Ok` reads stay best-effort); M-4 = new exit code 3 for Error-severity diagnostics; I-3 = context-dependent severity kept, rendering unified (full `path.display()`) via one shared `rcd-core` helper; replacement.rs = converted to diagnostics using the same threaded-`errors` pattern as commit f309072.

**Tech Stack:** Rust, quick-xml 0.37.5 (empirics: unclosed CDATA latches the reader so nested swallows can yield zero diagnostics; mid-tag truncation cascades — both verified), clap, existing Diagnostic/Report pipeline.

**Context notes for the implementer:**

- Work continues on branch `fix/p1-error-propagation` in worktree `/home/anten/code/rimworld/rcd-p1` (stacks on `02d05a7`). Build is warm.
- Current exit codes (`crates/rcd-cli/src/main.rs`): `0` clean, `1` conflict-Error (`report.has_errors()`, :140), `2` usage / "no mods found" (:97, :166). New: `3` = scan completed but Error-severity diagnostics exist. Precedence: 2 (total failure) > 1 (conflicts) > 3 (error diagnostics) > 0.
- Verified repros (do not re-derive): `rcd --format json scan --dir <only-broken>` → 0 bytes stdout, exit 2 (I-1). `parse_defs_from_xml(r#"<Defs><ThingDef><defName><![CDATA[Steel</defName><label>x</label></ThingDef></Defs>"#, "t.xml")` → `defs=0, errors=0` (I-4).
- Duplicate diagnostics per malformed file after I-4 (pre-check error + structural cascade error) are ACCEPTED: both are true statements; dedup plumbing isn't worth it. Note it in the commit message.
- Per AGENTS.md: `cargo test` + `cargo clippy --workspace --all-targets -- -D warnings` before every commit.

---

### Task 1: Well-formedness pre-check (I-4)

**Files:**

- Modify: `crates/rcd-xml/src/def_parser.rs` (new `pub(crate) fn check_well_formed`; call at top of `parse_defs_from_xml`)
- Modify: `crates/rcd-xml/src/patch_ops.rs:80` (call at top of `parse_patches_from_xml`)
- Test: both files' test modules

**Step 1: Write the failing tests**

Add to `mod tests` in `def_parser.rs`:

```rust
#[test]
fn unclosed_cdata_yields_diagnostic_via_precheck() {
    // quick-xml latches on this error inside nested best-effort reads, so the
    // structural parser alone reports nothing; the pre-check must catch it.
    let xml = r#"<Defs><ThingDef><defName><![CDATA[Steel</defName><label>x</label></ThingDef></Defs>"#;
    let (_defs, _skipped, errors) = parse_defs_from_xml(xml, "t.xml");
    assert!(!errors.is_empty(), "pre-check must surface the malformed document");
    assert!(errors.iter().all(|e| e.contains("t.xml")));
}
```

Add the symmetric test to `patch_ops.rs`:

```rust
#[test]
fn unclosed_cdata_in_xpath_yields_diagnostic_via_precheck() {
    let xml = r#"<Patch><Operation Class="PatchOperationReplace"><xpath><![CDATA[Defs/ThingDef</xpath><value><label>x</label></value></Operation></Patch>"#;
    let (_ops, errors) = parse_patches_from_xml(xml, "t.xml");
    assert!(!errors.is_empty(), "pre-check must surface the malformed document");
    assert!(errors.iter().all(|e| e.contains("t.xml")));
}
```

**Step 2: Run to verify they fail**

Run: `cargo test -p rcd-xml unclosed_cdata`
Expected: FAIL (errors empty)

**Step 3: Implement**

In `def_parser.rs` (module level, `pub(crate)` so patch_ops can use it):

```rust
/// One-event-per-line scan that guarantees a diagnostic for any malformed
/// document, even when structural parsing swallows the error in a best-effort
/// nested read (quick-xml can latch after Err, hiding it from outer loops).
/// Returns Some(message) when the document is not well-formed.
pub(crate) fn check_well_formed(xml: &str, source: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Eof) => return None,
            Ok(_) => {}
            Err(e) => {
                return Some(format!(
                    "XML parse error in {}: {} (document not well-formed; results may be incomplete)",
                    source, e
                ));
            }
        }
        buf.clear();
    }
}
```

At the top of `parse_defs_from_xml` (after `let mut errors = Vec::new();`):

```rust
    if let Some(msg) = check_well_formed(xml, source) {
        errors.push(msg);
    }
```

Same two lines at the top of `parse_patches_from_xml` in `patch_ops.rs`, with `use crate::def_parser::check_well_formed;`.

**Step 4: Verify**

Run: `cargo test && cargo clippy --workspace --all-targets -- -D warnings`
Expected: all PASS (structural captures still fire too — mid-tag truncation may now yield 2 diagnostics per file; accepted).

**Step 5: Commit**

```bash
git add crates/rcd-xml/
git commit -m "xml: guarantee malformed-document diagnostics via well-formedness pre-check

Nested best-effort reads (if let Ok(Event::Text..)) discard Err, and
quick-xml 0.37.5 can latch after certain errors (unclosed CDATA), so the
structural parser could yield zero defs AND zero diagnostics. A one-pass
event scan at each entry point now guarantees visibility. A malformed file
may report twice (pre-check + structural cascade); both statements are
true and dedup plumbing is not worth it."
```

---

### Task 2: replacement.rs error capture (reviewer NOTE)

**Files:**

- Modify: `crates/rcd-xml/src/replacement.rs` — `extract_child_tags` (~:180, Err at :207), `extract_tag_content` (~:216, Err at :266), `detect_replacements` return type + internals, its tests
- Modify: `crates/rcd-xml/src/lib.rs:104` (caller)
- Test: replacement.rs test module

**Step 1: Failing test**

Add to `mod tests` in `replacement.rs`:

```rust
#[test]
fn detect_replacements_reports_internal_reconstruction_errors() {
    // raw_xml is OUR reconstruction; a parse Err here means a reconstruction
    // bug, which must surface as a diagnostic rather than vanish. Well-formed
    // input must keep the errors vec empty.
    let mod_a = ModDefs {
        mod_summary: test_mod_summary_named("test.mod.a", "Mod A"),
        defs: vec![ParsedDef {
            def_type: "ThingDef".to_string(),
            def_name: "Steel".to_string(),
            raw_xml: "<ThingDef><defName>Steel</defName></ThingDef>".to_string(),
            source_file: "a.xml".to_string(),
        }],
    };
    let (conflicts, errors) = detect_replacements(&[mod_a.clone(), mod_a], None);
    assert!(conflicts.is_empty());
    assert!(errors.is_empty(), "well-formed reconstructed XML must not report: {errors:?}");
}
```

**Step 2: Run to verify it fails** — `cargo test -p rcd-xml detect_replacements_reports` → COMPILE FAIL (arity).

**Step 3: Implement** — same threaded pattern as f309072:

- `extract_child_tags(xml, target_tag, errors: &mut Vec<String>)` and `extract_tag_content(xml, target_tag, errors: &mut Vec<String>)`; their `Err(_) => break` becomes:

```rust
            Err(e) => {
                errors.push(format!(
                    "internal XML reconstruction error while diffing def content: {e}"
                ));
                break;
            }
```

- `detect_replacements(mods, order) -> (Vec<Conflict>, Vec<String>)`: create the errors vec, pass `&mut errors` to the two helpers at every call site, return both.
- Update `crates/rcd-xml/src/lib.rs:104`:

```rust
        let (replacement_conflicts, recon_errors) = replacement::detect_replacements(&all_mod_defs, order);
        all_conflicts.extend(replacement_conflicts);
        for msg in recon_errors {
            all_diagnostics.push(Diagnostic {
                affected_mod: None,
                severity: Severity::Warning,
                message: msg,
                source: DiagnosticSource::XmlParsing,
            });
        }
```

- Update all `detect_replacements(...)` test call sites in replacement.rs (and any in lib.rs tests) to destructure the tuple; assert `errors.is_empty()` in at least one.

**Step 4: Verify** — `cargo test && cargo clippy --workspace --all-targets -- -D warnings`

**Step 5: Commit**

```bash
git add crates/rcd-xml/
git commit -m "xml: capture parse errors on reconstructed def XML as diagnostics

replacement.rs still had two silent Err(_) => break sites (final review
NOTE). These parse our own raw_xml reconstruction, so an Err indicates a
reconstruction bug; it now flows through the same threaded-errors pattern
as def_parser/patch_ops, with a message that says so."
```

---

### Task 3: Unified About.xml-failure rendering + honest "not found" (I-3, I-2, M-1)

**Files:**

- Modify: `crates/rcd-core/src/diagnostic.rs` (new helper)
- Modify: `crates/rcd-core/src/folder.rs` (:84-94 M-1 match; :130-141 helper)
- Modify: `crates/rcd-cli/src/resolve.rs:40-45` (helper), `:114-119` (I-2 reword)
- Modify: `crates/rcd-ffi/src/lib.rs:58-66` (helper)
- Test: folder.rs test assertions (message now full path)

**Step 1: Update the existing test to the new expectation (failing)**

In `folder.rs` test `discover_mods_reports_unparseable_about_xml`, keep `assert!(diagnostics[0].message.contains("BrokenMod"));` (still true for full path) and ADD:

```rust
    assert!(
        diagnostics[0].message.contains(&dir.path().display().to_string()),
        "message must carry the full path so --config can distinguish Data/ vs Mods/ vs Workshop"
    );
```

**Step 2: Run to verify it fails** — `cargo test -p rcd-core discover_mods_reports` → FAIL (message has folder name only).

**Step 3: Implement the helper** in `crates/rcd-core/src/diagnostic.rs`:

```rust
/// One rendering for About.xml parse failures across CLI/core/FFI: full
/// Display path, consistent wording. Severity and source stay context-dependent
/// at the call sites (Error/MetadataResolution for explicitly-named paths,
/// Warning/FolderResolution for ambient directory discovery).
pub fn about_parse_failure(
    path: &std::path::Path,
    err: &impl std::fmt::Display,
    severity: crate::Severity,
    source: DiagnosticSource,
) -> Diagnostic {
    Diagnostic {
        affected_mod: None, // package_id is unknown — parsing failed
        severity,
        message: format!("Failed to parse mod at {}: {}", path.display(), err),
        source,
    }
}
```

Then:

- `folder.rs` parse-failure arm → `diagnostics.push(crate::about_parse_failure(&path, &e, crate::Severity::Warning, crate::DiagnosticSource::FolderResolution));` (drop the file_name-only formatting).
- `folder.rs` read_dir let-else (M-1) → explicit match so the real error is named:

```rust
        let entries = match std::fs::read_dir(search_dir) {
            Ok(entries) => entries,
            Err(e) => {
                diagnostics.push(crate::Diagnostic {
                    affected_mod: None,
                    severity: crate::Severity::Warning,
                    message: format!("Failed to read directory {}: {}", search_dir.display(), e),
                    source: crate::DiagnosticSource::FolderResolution,
                });
                continue;
            }
        };
```

- `resolve.rs:40-45` → `diagnostics.push(rcd_core::diagnostic::about_parse_failure(path, &e, Severity::Error, DiagnosticSource::MetadataResolution));` (kills the `{:?}` Debug-path variant).
- `rcd-ffi/src/lib.rs:58-66` → same helper call (Error/MetadataResolution).
- `resolve.rs:114-119` (I-2) reword:

```rust
                message: format!(
                    "Mod '{}' not found in search directories (a mod folder may have failed to parse — see FolderResolution diagnostics)",
                    package_id
                ),
```

**Step 4: Verify** — `cargo test && cargo clippy --workspace --all-targets -- -D warnings`. If the ffi test `rcd_analyze_reports_unparseable_mod_paths` asserts on message text, "Failed to parse mod" still matches — verify, don't rewrite blindly.

**Step 5: Commit**

```bash
git add crates/
git commit -m "core: one rendering for About.xml parse failures; honest not-found message

Final review I-3: the same failure was hand-rendered three ways (Debug-quoted
path, Display path, filename-only dropping the search dir). All three call
sites now share about_parse_failure with full Display path; severity stays
context-dependent by design. I-2: 'not found in search directories' was
factually wrong for a mod that WAS found but failed to parse — reworded.
M-1: read_dir failure now names the underlying io::Error."
```

---

### Task 4: End-to-end CLI resolution tests (M-3)

**Files:**

- Modify: `crates/rcd-cli/Cargo.toml` (add `[dev-dependencies] tempfile = { workspace = true }`)
- Modify: `crates/rcd-cli/src/resolve.rs` (add `#[cfg(test)] mod tests` — the file already has a tests module with `mark_base_content_identifies_ludeon_mods`; add there)

**Step 1: Write the tests** (they should pass immediately — these are coverage, not new behavior; red-first applies only if they reveal a bug):

```rust
#[test]
fn resolve_from_dir_surfaces_broken_mod_alongside_good_ones() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("GoodMod/About");
    std::fs::create_dir_all(&good).unwrap();
    std::fs::write(
        good.join("About.xml"),
        "<ModMetaData><packageId>test.good</packageId><name>Good</name><author>a</author><supportedVersions><li>1.5</li></supportedVersions></ModMetaData>",
    )
    .unwrap();
    let broken = dir.path().join("BrokenMod/About");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("About.xml"), "<ModMetaData><unclosed>").unwrap();

    let resolved = resolve_from_dir(dir.path(), Some("1.5"));
    assert_eq!(resolved.mods.len(), 1);
    assert_eq!(resolved.mods[0].package_id, "test.good");
    assert_eq!(resolved.diagnostics.len(), 1);
    assert_eq!(
        resolved.diagnostics[0].source,
        rcd_core::DiagnosticSource::FolderResolution
    );
}

#[test]
fn resolve_from_config_broken_mod_reports_parse_warning_and_not_found_pair() {
    // Pins the I-2 behavior: a broken mod that the config references yields
    // BOTH the FolderResolution parse warning and the (reworded) not-found
    // warning — the pair is the documented correlation, not a bug.
    let root = tempfile::tempdir().unwrap();
    let mods_dir = root.path().join("Mods");
    let broken = mods_dir.join("BrokenMod/About");
    std::fs::create_dir_all(&broken).unwrap();
    std::fs::write(broken.join("About.xml"), "<ModMetaData><unclosed>").unwrap();
    let config = root.path().join("ModsConfig.xml");
    std::fs::write(
        &config,
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ModsConfigData><version>1.5</version><activeMods><li>BrokenMod</li></activeMods></ModsConfigData>",
    )
    .unwrap();

    let resolved = resolve_from_config(&config, root.path(), Some("1.5"));
    assert!(resolved.mods.is_empty());
    assert_eq!(resolved.diagnostics.len(), 2, "parse warning + not-found warning");
    assert!(resolved.diagnostics.iter().any(|d| d.message.contains("Failed to parse mod")));
    assert!(resolved.diagnostics.iter().any(|d| d.message.contains("may have failed to parse")));
}
```

If the exact About.xml/ModsConfig.xml shapes don't parse with the real parsers, adjust fixtures to match `about_xml.rs`/`mods_config.rs` accepted formats (read those parsers — do not guess twice).

**Step 2: Verify** — `cargo test && cargo clippy --workspace --all-targets -- -D warnings`

**Step 3: Commit**

```bash
git add crates/rcd-cli/
git commit -m "cli: add end-to-end tests for resolution diagnostics

The resolve -> ResolvedMods.diagnostics path had zero coverage (rcd-cli had
no tests). Pins the good+broken mix for --dir and the documented
parse-warning + not-found pair for --config."
```

---

### Task 5: JSON total-failure report + ordering + exit-code ladder (I-1, M-2, M-4)

**Files:**

- Modify: `crates/rcd-core/src/report.rs` (new `has_error_diagnostics`)
- Modify: `crates/rcd-cli/src/main.rs` (:92-98 I-1, :119 M-2, :140 M-4)
- Modify: `crates/rcd-cli/src/args.rs` (help epilog documenting the ladder)
- Test: report.rs unit test

**Step 1: Failing test** in `report.rs` tests:

```rust
#[test]
fn has_error_diagnostics_ignores_warnings_and_conflicts() {
    let diag = |sev: crate::Severity| crate::Diagnostic {
        affected_mod: None,
        severity: sev,
        message: "m".into(),
        source: crate::DiagnosticSource::MetadataResolution,
    };
    let report = Report::build(
        ReportMetadata { timestamp: "t".into(), game_version: None, order_aware: false, mods_scanned: vec![] },
        vec![],
        vec![diag(crate::Severity::Warning), diag(crate::Severity::Info)],
    );
    assert!(!report.has_error_diagnostics());
    let report = Report::build(
        ReportMetadata { timestamp: "t".into(), game_version: None, order_aware: false, mods_scanned: vec![] },
        vec![],
        vec![diag(crate::Severity::Error)],
    );
    assert!(report.has_error_diagnostics());
}
```

**Step 2: Verify it fails** (method doesn't exist).

**Step 3: Implement**

`report.rs`:

```rust
    /// True if any diagnostic is Error-severity (resolution/parse failures),
    /// independent of conflict severities. Drives CLI exit code 3.
    pub fn has_error_diagnostics(&self) -> bool {
        self.diagnostics.iter().any(|d| d.severity == Severity::Error)
    }
```

`main.rs` — restructure `run_scan` so total failure still emits a report:

```rust
    // (remove the early-return block at :92-98 entirely)
    // ... engine/metadata/report build unchanged, but metadata uses resolved.mods
    // as before (empty vec is fine — detectors handle empty input) ...

    // Merge resolution diagnostics FIRST so consumers see load failures before
    // conflict data (matches FFI splice ordering).
    report.diagnostics.splice(0..0, resolved.diagnostics);

    // Output based on format (unchanged) ...

    if report.metadata.mods_scanned.is_empty() && !report.diagnostics.is_empty() {
        2 // total resolution failure — JSON already on stdout with the diagnostics
    } else if report.has_errors() {
        1
    } else if report.has_error_diagnostics() {
        3
    } else {
        0
    }
```

`args.rs` — add/extend the clap `about`/`after_help` on `Cli`:

```
Exit codes: 0 clean, 1 conflicts at Error severity, 2 no mods found / usage,
3 scan completed with Error-severity diagnostics (partial results).
```

**Step 4: Verify**

Run: `cargo test && cargo clippy --workspace --all-targets -- -D warnings`
Then the I-1 repro end-to-end:

```bash
mkdir -p /tmp/rcd-broken2/BrokenMod/About && printf '<ModMetaData><unclosed>' > /tmp/rcd-broken2/BrokenMod/About/About.xml
cargo run -q -p rcd-cli -- --format json scan --dir /tmp/rcd-broken2 > /tmp/o.json 2>/dev/null; echo "exit=$?"
python3 -c "import json; r=json.load(open('/tmp/o.json')); assert r['metadata']['mods_scanned']==[]; assert any('Failed to parse mod' in d['message'] for d in r['diagnostics']); print('I-1 OK')"
```

Expected: `exit=2`, `I-1 OK`. Also check README: `rg -n 'exit' README.md` — if exit codes are documented anywhere, update; if not, the `after_help` is the contract.

**Step 5: Commit**

```bash
git add crates/rcd-cli/ crates/rcd-core/src/report.rs
git commit -m "cli: emit JSON report on total resolution failure; add exit code 3

I-1: --format json with every mod broken produced empty stdout + exit 2 —
the diagnostics existed but only on stderr, diverging from FFI which returns
a full report in the same case. run_scan now always builds and prints the
report; exit 2 still signals total failure. M-2: CLI prepends resolution
diagnostics like FFI. M-4: exit code 3 = completed scan with Error-severity
diagnostics; ladder documented in --help."
```

---

### Task 6: TODO hygiene + final verification (M-5)

**Files:**

- Modify: `TODO.md` (stale line refs in the four flipped Error-Handling bullets: `folder.rs:81,98` → `folder.rs:84-141`; `rcd-ffi/src/lib.rs:54` → `rcd-ffi/src/lib.rs:55-66,101`; add a one-line note under the XML-parse-errors bullet that nested best-effort reads are covered by the pre-check, not per-site capture)

**Steps:**

1. Apply TODO edits.
2. Full gate: `cargo test && cargo clippy --workspace --all-targets -- -D warnings`.
3. Re-run the I-4 CDATA repro as a one-off (`cargo test -p rcd-xml unclosed_cdata`) — expect PASS (covered by Task 1 tests).
4. Commit:

```bash
git add TODO.md
git commit -m "docs: refresh TODO line refs after final-review fixes

M-5: the flipped P1 bullets cited pre-branch positions. No history rewrite
for the commit-label hygiene findings; noted for future branches."
```

---

## Out of scope (deliberate)

- **M-5 commit-history hygiene** (d1f33dd `cli:` label covering core changes; f309072/339251c split) — rewriting published-review-tested history for labels isn't worth the re-review cost.
- **T1 Err-arm test seam** — still deferred pending user decision.
- **P0 items** (XPathOnReplacedDef break, empty-defName desync, root attrs, FFI is_base_content) — separate plan.
- **`replacement.rs` beyond the two Err sites** — its normalize_whitespace fallback at :124 is a value-formatting concern, not a silent failure.

## Definition of done

- `cargo test` green (existing + ~7 new tests), `cargo clippy --workspace --all-targets -- -D warnings` clean
- I-1 repro: `--format json` all-broken → valid JSON on stdout with diagnostics, exit 2
- I-4 repro: unclosed CDATA → non-empty errors on both parsers
- `--config` broken-mod pair: second message no longer claims plain "not found"
- Three About.xml-failure renderings byte-identical modulo severity/source
- Exit-code ladder documented in `--help`
- Final-review findings I-1..I-4, M-1..M-4 all resolved; M-5 partially (TODO refs) with history rewrite explicitly declined
