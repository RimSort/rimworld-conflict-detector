# Task 1 report — XPath conflict completeness and aggregation

## Result

Implemented complete XPath-on-replaced-def findings and aggregate same-concrete-def non-overlap Info conflicts. The new mechanism is public and maps to `Info`; `XPathOnReplacedDef` remains `Error`. The aggregate is one conflict per `(def_type, def_name, distinct package-ID pair)`, with package IDs and XPath orientation canonicalized lexically, and unique sorted XPath pairs. Wildcard/different-def operations and overlapping pairs do not enter this aggregate. Existing overlap findings remain separately emitted.

## TDD evidence

Added the behavioral tests before implementing the producer behavior. The first RED run was:

```text
$ cargo test -p rcd-xml --lib conflict::tests::reports_every_distinct_replacer_for_a_patcher
running 1 test
test conflict::tests::reports_every_distinct_replacer_for_a_patcher ... FAILED
assertion `left == right` failed
  left: 1
 right: 3
```

The broader RED run was `cargo test -p rcd-xml --lib conflict::tests::`: 4 failed, 11 passed, 1 ignored. The four failures covered only-one-replacer reporting, self-replacer completeness, missing aggregate behavior, and excluded-pair/aggregation behavior. After implementation, the same focused module run passed: 15 passed, 0 failed, 1 ignored. `rcd-core` conflict tests also passed: 10 passed, 0 failed.

Tests added/updated:
- `reports_every_distinct_replacer_for_a_patcher`: checks three replacement package IDs and Error severity.
- `excludes_patcher_from_replacer_conflicts`: checks self-exclusion and each of two other replacers once.
- `aggregates_unique_non_overlapping_pairs_for_same_concrete_def`: checks aggregation, stable package/pair ordering, duplicate XPath-pair removal, and Info severity.
- `non_overlap_info_excludes_wildcard_different_defs_and_overlapping_pairs`: covers wildcard name, different def type and name, and a retained actual-overlap case.
- Updated prior non-overlap and bucket-order assertions to account for the new Info result without pinning unrelated output ordering.
- `same_def_non_overlapping_xpaths_are_info` in core verifies severity mapping.

## Interface and staging note

Added `DefConflictMechanism::XPathSameDefNonOverlap { xpath_pairs: Vec<(String, String)> }` and its Info severity in `crates/rcd-core/src/conflict.rs`, as assigned to Task 1 by the preflight ledger. Workspace testing then revealed an exhaustive CLI match compile failure in `crates/rcd-cli/src/output.rs`. Supervisor approved a narrow staging adjustment: an explicit label-only match arm (`same def, non-overlapping XPath patches`) restores exhaustive compilation; detailed XPath rendering and schema/docs remain Task 2. No wildcard arm, schema change, or docs change was added.

## Validation

- RED: `cargo test -p rcd-xml --lib conflict::tests::reports_every_distinct_replacer_for_a_patcher` — failed as expected (1 replacing mod found rather than 3).
- RED: `cargo test -p rcd-xml --lib conflict::tests::` — 4 failed / 11 passed / 1 ignored before producer implementation.
- GREEN: `cargo test -p rcd-xml --lib conflict::tests::` — 15 passed / 0 failed / 1 ignored.
- GREEN: `cargo test -p rcd-core --lib conflict::tests::` — 10 passed / 0 failed.
- `cargo test --workspace` — passed; 93 XML tests passed, 1 ignored, and all other workspace unit/doc test groups passed.
- `cargo clippy --workspace --all-targets -- -D warnings` — passed.
- `cargo fmt --all -- --check` — passed.
- `git diff --check` — passed.

A workspace test attempt before the approved CLI match arm failed on that exhaustive match, and an initial clippy run identified a type-complexity lint; the explicit CLI arm and local aliases resolved these. Final workspace tests and final clippy both passed.

## Files

- `crates/rcd-xml/src/conflict.rs` — removed the replacement-loop early break; added deterministic aggregation and behavioral tests.
- `crates/rcd-core/src/conflict.rs` — added the mechanism, Info severity mapping, and unit test.
- `crates/rcd-cli/src/output.rs` — supervisor-approved minimal explicit label arm required for exhaustive compilation; no detailed rendering changes.
- `.superpowers/sdd/2026-10-13-conflict-cli-contracts-plan/task-1-report.md` — this report.

## Residual risks / out of scope

Detailed aggregate XPath display, serialized report schema version 2, CLI JSON/report contracts, README/TODO changes, and other Task 2 work remain out of scope. No source cleanup outside the assigned behavior was performed. The existing unrelated worktree modifications and untracked plan files were not changed or staged.