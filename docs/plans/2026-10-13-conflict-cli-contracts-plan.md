# Conflict and CLI Report Contracts Implementation Plan

> **For agentic workers:** Use the approved design at `docs/plans/2026-10-13-correctness-loadfolders-design.md`. This plan owns public conflict/report and list-mods output contracts.

**Goal:** Report all applicable replacement-patch conflicts, expose the requested aggregate Info result without uncontrolled report growth, and make `list-mods` resolution diagnostics machine-readable.

**Architecture:** Remove the lossy early exit in XPath-on-replaced-def detection. Represent same-def non-overlap findings as a distinct aggregate conflict, advance the serialized report schema, and use one stable JSON envelope for `list-mods` in both success and failure cases.

**Tech Stack:** Rust 2024, serde/serde_json, existing CLI report and diagnostic types.

**Spec:** `docs/plans/2026-10-13-correctness-loadfolders-design.md`

## Global Constraints

- `XPathOnReplacedDef` remains Error severity; non-overlap findings are Info.
- Aggregate exactly one non-overlap conflict per concrete def and distinct mod pair, with unique deterministic XPath pairs; exclude wildcard/different-def pairs.
- Increment report `schema_version` to 2 because `DefConflictMechanism` gains a public serialized variant.
- `list-mods --format json` always emits `{ "mods": [...], "diagnostics": [...] }`; preserve human output and the exit-code ladder.
- No push or merge to `main`; commits after verification/review are authorized.

## Review Focus

- A patcher distinct from three replacers yields all three `XPathOnReplacedDef` conflicts.
- A patcher that is itself one of the replacers does not create a self-conflict.
- Repeated patch operations aggregate deterministically without losing underlying XPath pair details.
- Wildcards and different defs do not leak into the same-def non-overlap Info set.
- Empty and total-failure `list-mods` JSON outputs remain parseable and include diagnostics.

---

### Task 1: XPath replacement conflict completeness and Info aggregation

**Files:** Modify `crates/rcd-xml/src/conflict.rs`, `crates/rcd-core/src/conflict.rs`, and tests in those modules.

**Interfaces:** Add `DefConflictMechanism::XPathSameDefNonOverlap { xpath_pairs: Vec<(String, String)> }` and its Info severity mapping here. The CLI renderer and report schema are updated in Task 2.

- [ ] Add a failing test with three distinct replacement mods plus a fourth patching mod; assert exactly one conflict for each replacing mod and the complete `replaced_by` set.
- [ ] Add a test where the patching mod is also a replacer; assert the patcher is excluded and every other distinct replacer is included once.
- [ ] Add failing tests for two mods patching different fields of the same concrete def, several non-overlapping pairs aggregating to one Info, and wildcard/different-def/overlap pairs being excluded.
- [ ] Remove the premature `break`; rely on the replacement map's per-package deduplication.
- [ ] Implement same-def grouping and emit one aggregate conflict per concrete def and distinct mod pair, with unique sorted XPath pairs. Do not emit one Info per operation pair.
- [ ] Verify focused tests pass and avoid assertions on unrelated HashMap-derived global conflict order.

### Task 2: Public report schema, CLI envelope, and documentation

**Files:** Modify `crates/rcd-core/src/report.rs`, `crates/rcd-cli/src/main.rs`, `crates/rcd-cli/src/output.rs`, `README.md`, and `TODO.md` after the behavior is verified.

**Interfaces:** New conflict tag is `xpath_same_def_non_overlap`; report `schema_version` becomes 2. `list-mods --format json` always emits `{ "mods": [ModSummary, ...], "diagnostics": [Diagnostic, ...] }`.

- [ ] Add tests for serialization of the new mechanism; pin report schema version 2.
- [ ] Add failing CLI tests for successful JSON output, zero-mod output with diagnostics, and partial resolution with mods plus warnings.
- [ ] Implement human conflict rendering with enough XPath detail to explain the aggregate.
- [ ] Change the JSON list formatter to serialize the envelope; human format continues showing resolution diagnostics on stderr and existing list text.
- [ ] Ensure JSON mode writes the envelope to stdout before returning the existing exit code, with no non-JSON stderr prose for the empty-result case.
- [ ] Update README for report schema version 2 and the `list-mods --format json` envelope.
- [ ] Mark only implemented correctness/folder items complete in TODO.md; do not check off deferred or excluded cleanup items.
- [ ] Verify focused tests, CLI tests, report serialization tests, and crate/workspace tests pass.
