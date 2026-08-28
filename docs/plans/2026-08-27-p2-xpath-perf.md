# P2: XPath Conflict Detection Performance Implementation Plan

> **REQUIRED SUB-SKILL:** Use the executing-plans skill to implement this plan task-by-task.

**Goal:** Fix TODO P2 items #9–11: reduce XPath conflict detection from O(n²) with redundant normalization to O(n + Σ k²-per-def-type), make `LoadOrder::position` O(1), and parallelize the pairwise comparison with rayon — targeting the 739-mod / ~27M-comparison workload.

**Architecture:** `detect_xpath_conflicts` (`crates/rcd-xml/src/conflict.rs:20-136`) currently flattens every patch op from every mod into `all_ops` and does a raw i<j nested loop, calling `normalize_xpath` twice per pair. Two facts make grouping safe: (1) `xpaths_overlap` returns `false` unless both ops have the *same* `def_type` (`Some` and equal), so ops can be bucketed by def_type and only compared within a bucket; (2) ops with `def_type == None` can never overlap anything and can be skipped entirely. Normalization happens once per op before bucketing. `LoadOrder::position` becomes a `HashMap` lookup behind a `LoadOrder::new()` constructor; the private map field makes every struct-literal construction site a compiler error, so migration is compiler-driven.

**Tech Stack:** Rust, rayon 1 (already a workspace dep of rcd-xml), std HashMap.

**Context notes for the implementer:**

- Behavior contract: the *set* of detected conflicts must not change. Output *order* will change (grouped by def_type instead of flattened op order) — existing tests assert existence/counts, not order; if you find one that does, convert it to a set/sorted assertion.
- All 47 tests green at plan time; `cargo test && cargo clippy --workspace -- -D warnings` before every commit (AGENTS.md).
- Worktree: `git worktree add ../rcd-p2 -b perf/p2-xpath-detection`.
- If the P1 plan landed first on a sibling branch, `patch_ops.rs` signatures may already have changed — rebase and re-read before editing.

---

### Task 1: O(1) `LoadOrder::position` via HashMap (TODO P2 #10)

**Why first:** Independent of Task 2, small, and Task 2's inner loop calls `position` twice per pair — so this must land first or Task 2's speedup is partially negated.

**Design:** Add a private `positions: HashMap<String, usize>` built in `LoadOrder::new()`. Keep `package_ids` public (it's read in `validate_load_order` and serialized metadata paths). Duplicate package_ids preserve current first-occurrence-wins semantics (`Vec::position` returns the first match). Because the map field is private, all 14 struct-literal construction sites fail to compile — that's the migration mechanism, not an accident. (Verified by workspace-wide grep: 3 production + 11 test sites; the list below is complete.)

**Files:**

- Modify: `crates/rcd-core/src/types.rs:42-57` (`LoadOrder`)
- Modify construction sites (compiler will list them):
  - Production: `crates/rcd-core/src/mods_config.rs:13-16`, `crates/rcd-cli/src/resolve.rs:52-54`, `crates/rcd-ffi/src/lib.rs:61-63`
  - Tests: `crates/rcd-core/src/types.rs:66, :76`, `crates/rcd-core/src/load_order.rs:115, :131, :145`, `crates/rcd-core/tests/integration.rs:54`, `crates/rcd-xml/src/replacement.rs:380`, `crates/rcd-xml/src/conflict.rs:343`, `crates/rcd-harmony/src/conflict.rs:387, :450, :573`

**Step 1: Write the failing test**

Add to `mod tests` in `crates/rcd-core/src/types.rs`:

```rust
#[test]
fn load_order_duplicate_package_id_first_occurrence_wins() {
    let order = LoadOrder::new(vec!["core".into(), "mod.a".into(), "mod.a".into()]);
    assert_eq!(order.position("mod.a"), Some(1), "must match Vec::position semantics");
}

#[test]
fn load_order_position_matches_vec_semantics() {
    let ids = vec!["a".into(), "b".into(), "c".into()];
    let order = LoadOrder::new(ids.clone());
    for (i, id) in ids.iter().enumerate() {
        assert_eq!(order.position(id), Some(i));
    }
    assert_eq!(order.position("missing"), None);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test -p rcd-core load_order`
Expected: COMPILE FAIL (`LoadOrder::new` doesn't exist)

**Step 3: Implement**

In `crates/rcd-core/src/types.rs`:

```rust
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct LoadOrder {
    pub package_ids: Vec<String>,
    positions: HashMap<String, usize>,
}

impl LoadOrder {
    pub fn new(package_ids: Vec<String>) -> Self {
        let mut positions = HashMap::with_capacity(package_ids.len());
        for (i, id) in package_ids.iter().enumerate() {
            // First occurrence wins, matching the old Vec::position behavior.
            positions.entry(id.clone()).or_insert(i);
        }
        Self {
            package_ids,
            positions,
        }
    }

    pub fn position(&self, package_id: &str) -> Option<usize> {
        self.positions.get(package_id).copied()
    }

    pub fn is_after(&self, a: &str, b: &str) -> Option<bool> {
        match (self.position(a), self.position(b)) {
            (Some(pa), Some(pb)) => Some(pa > pb),
            _ => None,
        }
    }
}
```

**Step 4: Migrate all construction sites (compiler-driven)**

Run: `cargo build --workspace --tests 2>&1 | rg 'cannot construct|missing field|E0063'` and fix each site:

`LoadOrder { package_ids: ... }` → `LoadOrder::new(...)`. The four production sites, verbatim:

```rust
// crates/rcd-core/src/mods_config.rs:13-16
    pub fn load_order(&self) -> LoadOrder {
        LoadOrder::new(self.active_mods.clone())
    }
```

```rust
// crates/rcd-cli/src/resolve.rs:52-54
        Some(LoadOrder::new(
            mods.iter().map(|m| m.package_id.clone()).collect(),
        ))
```

```rust
// crates/rcd-ffi/src/lib.rs:61-63
            Some(rcd_core::LoadOrder::new(
                mods.iter().map(|m| m.package_id.clone()).collect(),
            ))
```

Test sites: same mechanical substitution.

**Step 5: Verify**

Run: `cargo test && cargo clippy --workspace -- -D warnings`
Expected: all PASS, zero behavior change.

**Step 6: Commit**

```bash
git add -A
git commit -m "core: O(1) LoadOrder::position via HashMap

position() was a linear Vec scan called twice per overlapping pair inside
detect_xpath_conflicts, making winner determination O(n^3) worst case on
large mod lists. LoadOrder now carries a position map built in new();
duplicate ids keep first-occurrence-wins Vec::position semantics. The
private map field makes the compiler enumerate every construction site."
```

---

### Task 2: Pre-normalize and bucket by def_type (TODO P2 #9)

**Why:** The i<j loop over `all_ops` is n²/2 pairs and calls `normalize_xpath` (string splitting + predicate parsing) twice per pair. Bucketing by `def_type` is exact — `xpaths_overlap` requires equal `Some` def_types — so cross-bucket pairs are provably non-overlapping and can be skipped. Normalization drops from O(n²) calls to exactly n.

Complexity: n²/2 pairs → Σ_b k_b²/2 where k_b is bucket size. For the measured 739-mod workload (patches spread across many def types) this is a ~100-1000× reduction in overlap checks; worst case (all patches target one def type) is parity with today plus the normalization win.

**Files:**

- Modify: `crates/rcd-xml/src/conflict.rs:20-136` (`detect_xpath_conflicts`, section 1 only — leave section 2 / XPathOnReplacedDef alone)
- Test: `crates/rcd-xml/src/conflict.rs` (`mod tests`)

**Step 1: Write the failing/locking tests**

These lock behavior that the refactor must preserve. Add to `mod tests` in `crates/rcd-xml/src/conflict.rs`:

```rust
#[test]
fn ops_without_def_type_never_conflict() {
    // XPaths that normalize to def_type = None can't overlap anything.
    let patches = vec![
        (
            test_mod_summary_named("mod.a", "Mod A"),
            vec![ParsedPatchOp {
                op_type: PatchOpType::Replace,
                xpath: "Defs/statBases".to_string(), // no *Def segment → def_type None
                source_file: "mod_a/Patches/P.xml".to_string(),
                condition: None,
            }],
        ),
        (
            test_mod_summary_named("mod.b", "Mod B"),
            vec![ParsedPatchOp {
                op_type: PatchOpType::Replace,
                xpath: "Defs/ThingDef/statBases".to_string(),
                source_file: "mod_b/Patches/P.xml".to_string(),
                condition: None,
            }],
        ),
    ];
    let conflicts = detect_xpath_conflicts(&patches, &HashMap::new(), None);
    assert!(conflicts.is_empty());
}

#[test]
fn different_def_types_never_conflict_even_with_same_field() {
    let patches = vec![
        (
            test_mod_summary_named("mod.a", "Mod A"),
            vec![ParsedPatchOp {
                op_type: PatchOpType::Replace,
                xpath: "Defs/ThingDef/statBases".to_string(),
                source_file: "mod_a/Patches/P.xml".to_string(),
                condition: None,
            }],
        ),
        (
            test_mod_summary_named("mod.b", "Mod B"),
            vec![ParsedPatchOp {
                op_type: PatchOpType::Replace,
                xpath: "Defs/PawnKindDef/statBases".to_string(),
                source_file: "mod_b/Patches/P.xml".to_string(),
                condition: None,
            }],
        ),
    ];
    let conflicts = detect_xpath_conflicts(&patches, &HashMap::new(), None);
    assert!(conflicts.is_empty());
}
```

(Verified: the test module imports `test_mod_summary_named` from `rcd_core::testutil` and `PatchOpType`/`PatchCondition` from `crate::patch_ops`; there is NO `patch_op` helper — construct `ParsedPatchOp` inline as shown.)

**Step 2: Run tests to establish the baseline**

Run: `cargo test -p rcd-xml conflict`
Expected: PASS on current code too (these pin existing semantics — they're regression guards, not red-first tests; the red-first signal for this task is the benchmark in Step 6).

**Step 3: Refactor section 1 of `detect_xpath_conflicts`**

Replace the flat loop (lines ~30-104) with:

```rust
    // 1. Normalize every op once, then bucket by def_type.
    //    xpaths_overlap requires equal Some def_types, so only ops in the
    //    same bucket can conflict; ops with no def_type can't conflict at all.
    let normalized: Vec<(&ModSummary, &ParsedPatchOp, NormalizedXPath)> = all_ops
        .iter()
        .map(|(ms, op)| (*ms, *op, normalize_xpath(&op.xpath)))
        .collect();

    let mut buckets: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, (_, _, norm)) in normalized.iter().enumerate() {
        if let Some(dt) = norm.def_type.as_deref() {
            buckets.entry(dt).or_default().push(i);
        }
    }

    // Deterministic output: process buckets in sorted def_type order.
    let mut bucket_keys: Vec<&str> = buckets.keys().copied().collect();
    bucket_keys.sort_unstable();

    for key in bucket_keys {
        let indices = &buckets[key];
        for ai in 0..indices.len() {
            for bi in (ai + 1)..indices.len() {
                let (mod_a, op_a, norm_a) = &normalized[indices[ai]];
                let (mod_b, op_b, norm_b) = &normalized[indices[bi]];

                if mod_a.package_id == mod_b.package_id {
                    continue;
                }
                if !xpaths_overlap(norm_a, norm_b) {
                    continue;
                }

                let def_type = norm_a.def_type.clone().unwrap_or_default();
                let def_name = norm_a
                    .def_name
                    .clone()
                    .or_else(|| norm_b.def_name.clone())
                    .unwrap_or_default();

                let winner = order.and_then(|lo| {
                    let pos_a = lo.position(&mod_a.package_id);
                    let pos_b = lo.position(&mod_b.package_id);
                    match (pos_a, pos_b) {
                        (Some(pa), Some(pb)) => Some(if pa > pb {
                            mod_a.package_id.clone()
                        } else {
                            mod_b.package_id.clone()
                        }),
                        _ => None,
                    }
                });

                let condition = op_a
                    .condition
                    .as_ref()
                    .or(op_b.condition.as_ref())
                    .map(|c| format!("requires mod: {}", c.required_mod));

                let wildcard = norm_a.def_name.is_none() || norm_b.def_name.is_none();

                conflicts.push(Conflict::Def(DefConflict {
                    target: DefTarget { def_type, def_name },
                    mods: vec![(**mod_a).clone(), (**mod_b).clone()],
                    mechanism: DefConflictMechanism::XPathOverlap {
                        xpath_a: op_a.xpath.clone(),
                        xpath_b: op_b.xpath.clone(),
                        wildcard,
                    },
                    winner,
                    condition,
                }));
            }
        }
    }
```

Add `use crate::xpath::NormalizedXPath;` if not already imported. Leave section 2 (XPathOnReplacedDef) untouched.

Note the subtle delta from the old code: `def_name` resolution. Old code took `norm_a.def_name.or(norm_b.def_name)` for the *target*; keep that exactly. Old `def_type` was `norm_a.def_type.or(norm_b.def_type)` — inside one bucket both are `Some` and equal, so `norm_a.def_type.clone()` is identical.

**Step 4: Verify behavior**

Run: `cargo test && cargo clippy --workspace -- -D warnings`
Expected: all PASS. Same conflict set; order is now grouped by def_type (sorted), which no current test asserts against.

**Step 5: Commit**

```bash
git add crates/rcd-xml/src/conflict.rs
git commit -m "xml: bucket XPath conflict detection by def_type, normalize once

detect_xpath_conflicts compared every op pair (O(n^2)) and re-normalized
both XPaths per pair. xpaths_overlap requires equal Some def_types, so
ops are now normalized exactly once and only compared within their
def_type bucket. Ops without a def_type are skipped outright. Buckets
iterate in sorted order to keep report output deterministic."
```

**Step 6 (verification, not release-gated): Measure**

Add a temporary `#[ignore]` benchmark test to `crates/rcd-xml/src/conflict.rs`:

```rust
#[test]
#[ignore]
fn xpath_conflict_detection_large_scale_smoke() {
    // 200 mods x 30 patches each, spread over 50 def types.
    let mut patches = Vec::new();
    for m in 0..200 {
        let ms = test_mod_summary_named(&format!("mod.{m}"), &format!("Mod {m}"));
        let ops: Vec<_> = (0..30)
            .map(|i| ParsedPatchOp {
                op_type: PatchOpType::Replace,
                xpath: format!(
                    "Defs/ThingDef[defName=\"Item_{}_{}\"]/statBases",
                    i % 50,
                    i
                ),
                source_file: format!("mod.{m}/Patches/P.xml"),
                condition: None,
            })
            .collect();
        patches.push((ms, ops));
    }

    let start = std::time::Instant::now();
    let conflicts = detect_xpath_conflicts(&patches, &HashMap::new(), None);
    let elapsed = start.elapsed();
    eprintln!("6000 ops -> {} conflicts in {:?}", conflicts.len(), elapsed);
    assert!(elapsed.as_secs() < 5, "regression: took {:?}", elapsed);
}
```

Run: `cargo test -p rcd-xml --release -- --ignored xpath_conflict_detection_large_scale_smoke`
Record before/after numbers (check out the parent commit for the "before" run). Expect sub-second after vs several seconds before. Keep the test (`#[ignore]` keeps it out of the default run); mention the numbers in the PR description.

---

### Task 3: Parallelize buckets with rayon (TODO P2 #11)

**Why last:** Depends on Task 2's bucket structure. Each bucket is pure computation over immutable input → embarrassingly parallel. rayon is already a dependency of rcd-xml.

**Determinism contract:** buckets run in parallel but results are collected per-bucket and concatenated in the same sorted-key order as Task 2, and within a bucket the i<j loop stays sequential — so output order is identical to Task 2's.

**Files:**

- Modify: `crates/rcd-xml/src/conflict.rs` (the bucket loop from Task 2)

**Step 1: Refactor**

```rust
use rayon::prelude::*;

    let conflict_buckets: Vec<Vec<Conflict>> = bucket_keys
        .par_iter()
        .map(|key| {
            let indices = &buckets[key];
            let mut bucket_conflicts = Vec::new();
            for ai in 0..indices.len() {
                for bi in (ai + 1)..indices.len() {
                    // ... body unchanged from Task 2, pushing into bucket_conflicts ...
                }
            }
            bucket_conflicts
        })
        .collect();

    for bucket in conflict_buckets {
        conflicts.extend(bucket);
    }
```

`ModSummary`, `ParsedPatchOp`, `NormalizedXPath`, `LoadOrder` are all `Send + Sync` (plain data + HashMap). If the borrow checker complains about `order: Option<&LoadOrder>` in the closure, it's `Send + Sync` too — no `AssertUnwindSafe` needed.

**Step 2: Verify**

Run: `cargo test && cargo clippy --workspace -- -D warnings`
Expected: all PASS. Run the `--ignored` smoke bench from Task 2 Step 6 again; note the delta in the PR description. Also run the full suite a few times — rayon can expose hidden data races, and deterministic-order assertions only exist implicitly.

**Step 3: Commit**

```bash
git add crates/rcd-xml/src/conflict.rs
git commit -m "xml: parallelize XPath conflict buckets with rayon

Each def_type bucket is pure computation over immutable inputs. Buckets
run in parallel via par_iter and are concatenated in sorted-key order,
so report output order is unchanged from the sequential version."
```

---

## Out of scope (deliberate)

- **`detect_replacements` in replacement.rs** — already grouped by `(def_type, def_name)` map; not the hot path.
- **Criterion benchmarks as a dev-dependency** — the `#[ignore]` smoke test covers regression detection without adding a dep (YAGNI). Promote to criterion only if perf tracking becomes routine.
- **`rcd-harmony` conflict detection** — separate pipeline, different bottlenecks; TODO item #11 scopes to "phase 2 conflict detection" (XML pairwise).

## Definition of done

- `cargo test` green (existing suite + 3 new unit tests)
- `cargo clippy --workspace -- -D warnings` clean
- Conflict set on the fixture suite byte-identical to pre-change (spot-check: run CLI on fixtures before/after, diff JSON with `jaq 'sort_by(.conflicts)'`)
- Smoke bench shows >10× improvement on the 6000-op synthetic workload
- TODO.md items #9, #10, #11 flipped to `[x]`
