// XPath conflict detection for RimWorld mod patches
//
// Cross-references patch operations from different mods to detect:
// 1. XPath overlap: two mods patch the same (or overlapping) XPath target
// 2. XPath on replaced def: a mod patches a def that another mod fully replaces

use std::collections::HashMap;

use rcd_core::{Conflict, DefConflict, DefConflictMechanism, DefTarget, LoadOrder, ModSummary};

use crate::patch_ops::ParsedPatchOp;
use crate::xpath::{normalize_xpath, xpaths_overlap, NormalizedXPath};

/// Detect XPath-related conflicts across all mods' patch operations.
///
/// - `all_patches`: each entry is a mod's summary paired with its parsed patch ops
/// - `replacement_defs`: map of `(def_type, def_name)` to the mods that define that def
///   (from `replacement::detect_replacements` — entries with 2+ mods indicate a replacement)
/// - `order`: optional load order for winner determination
pub fn detect_xpath_conflicts(
    all_patches: &[(ModSummary, Vec<ParsedPatchOp>)],
    replacement_defs: &HashMap<(String, String), Vec<ModSummary>>,
    order: Option<&LoadOrder>,
) -> Vec<Conflict> {
    let mut conflicts = Vec::new();

    // Flatten all patches with their mod info for cross-referencing
    let all_ops: Vec<(&ModSummary, &ParsedPatchOp)> = all_patches
        .iter()
        .flat_map(|(ms, ops)| ops.iter().map(move |op| (ms, op)))
        .collect();

    // 1. Check for XPath overlaps between patches from different mods.
    //    Normalize every op once, then bucket by def_type: xpaths_overlap
    //    requires equal Some def_types, so only ops in the same bucket can
    //    conflict; ops with no def_type can't conflict at all.
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

                // Only flag conflicts between different mods
                if mod_a.package_id == mod_b.package_id {
                    continue;
                }
                if !xpaths_overlap(norm_a, norm_b) {
                    continue;
                }

                // Build the def target from the normalized xpaths
                let def_type = norm_a.def_type.clone().unwrap_or_default();
                let def_name = norm_a
                    .def_name
                    .clone()
                    .or_else(|| norm_b.def_name.clone())
                    .unwrap_or_default();

                // Determine winner by load order if available
                let winner = order.and_then(|lo| {
                    let pos_a = lo.position(&mod_a.package_id);
                    let pos_b = lo.position(&mod_b.package_id);
                    match (pos_a, pos_b) {
                        (Some(pa), Some(pb)) => {
                            if pa > pb {
                                Some(mod_a.package_id.clone())
                            } else {
                                Some(mod_b.package_id.clone())
                            }
                        }
                        _ => None,
                    }
                });

                // Build condition string from either op's condition
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

    // 2. Check for patches targeting defs that are fully replaced by another mod
    for (mod_summary, op) in &all_ops {
        let norm = normalize_xpath(&op.xpath);

        if let (Some(def_type), Some(def_name)) = (&norm.def_type, &norm.def_name) {
            let key = (def_type.clone(), def_name.clone());

            if let Some(replacing_mods) = replacement_defs.get(&key) {
                // Only flag if one of the replacing mods is different from the patching mod
                for replacing_mod in replacing_mods {
                    if replacing_mod.package_id != mod_summary.package_id {
                        conflicts.push(Conflict::Def(DefConflict {
                            target: DefTarget {
                                def_type: def_type.clone(),
                                def_name: def_name.clone(),
                            },
                            mods: vec![(*mod_summary).clone(), replacing_mod.clone()],
                            mechanism: DefConflictMechanism::XPathOnReplacedDef {
                                xpath: op.xpath.clone(),
                                replaced_by: replacing_mod.package_id.clone(),
                            },
                            winner: None,
                            condition: op
                                .condition
                                .as_ref()
                                .map(|c| format!("requires mod: {}", c.required_mod)),
                        }));
                        // Only report once per replacing mod for this patch
                        break;
                    }
                }
            }
        }
    }

    conflicts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch_ops::{PatchCondition, PatchOpType};
    use rcd_core::testutil::test_mod_summary_named;

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

    #[test]
    fn xpath_overlap_produces_warning() {
        let mod_c = test_mod_summary_named("test.mod.c", "Mod C");
        let mod_d = test_mod_summary_named("test.mod.d", "Mod D");

        let patches = vec![
            (
                mod_c,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#.to_string(),
                    source_file: "mod_c/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
            (
                mod_d,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#.to_string(),
                    source_file: "mod_d/Patches/Patches.xml".to_string(),
                    condition: Some(PatchCondition {
                        required_mod: "Test Mod C".to_string(),
                    }),
                }],
            ),
        ];

        let replacement_defs = HashMap::new();
        let conflicts = detect_xpath_conflicts(&patches, &replacement_defs, None);

        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            Conflict::Def(dc) => {
                assert_eq!(dc.target.def_type, "ThingDef");
                assert_eq!(dc.target.def_name, "Steel");
                assert!(matches!(
                    &dc.mechanism,
                    DefConflictMechanism::XPathOverlap { .. }
                ));
                assert_eq!(conflicts[0].severity(), rcd_core::Severity::Warning);
            }
            _ => panic!("Expected Conflict::Def"),
        }
    }

    #[test]
    fn xpath_on_replaced_def_produces_error() {
        let mod_a = test_mod_summary_named("test.mod.a", "Mod A");
        let mod_c = test_mod_summary_named("test.mod.c", "Mod C");

        let patches = vec![(
            mod_c.clone(),
            vec![ParsedPatchOp {
                op_type: PatchOpType::Replace,
                xpath: r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#.to_string(),
                source_file: "mod_c/Patches/Patches.xml".to_string(),
                condition: None,
            }],
        )];

        let mut replacement_defs = HashMap::new();
        replacement_defs.insert(
            ("ThingDef".to_string(), "Steel".to_string()),
            vec![mod_a.clone(), mod_c.clone()],
        );

        let conflicts = detect_xpath_conflicts(&patches, &replacement_defs, None);

        // Should find that mod_c patches Steel, which mod_a replaces
        let xpath_replaced: Vec<_> = conflicts
            .iter()
            .filter(|c| {
                matches!(
                    c,
                    Conflict::Def(dc) if matches!(
                        &dc.mechanism,
                        DefConflictMechanism::XPathOnReplacedDef { .. }
                    )
                )
            })
            .collect();

        assert_eq!(
            xpath_replaced.len(),
            1,
            "Should detect patch on replaced def"
        );

        match &xpath_replaced[0] {
            Conflict::Def(dc) => {
                assert_eq!(dc.target.def_type, "ThingDef");
                assert_eq!(dc.target.def_name, "Steel");
                assert_eq!(xpath_replaced[0].severity(), rcd_core::Severity::Error);
                match &dc.mechanism {
                    DefConflictMechanism::XPathOnReplacedDef { replaced_by, .. } => {
                        assert_eq!(replaced_by, "test.mod.a");
                    }
                    _ => panic!("Expected XPathOnReplacedDef mechanism"),
                }
            }
            _ => panic!("Expected Conflict::Def"),
        }
    }

    #[test]
    fn no_conflict_within_same_mod() {
        let mod_c = test_mod_summary_named("test.mod.c", "Mod C");

        let patches = vec![(
            mod_c,
            vec![
                ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#.to_string(),
                    source_file: "mod_c/Patches/Patches.xml".to_string(),
                    condition: None,
                },
                ParsedPatchOp {
                    op_type: PatchOpType::Add,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/statBases"#.to_string(),
                    source_file: "mod_c/Patches/Patches.xml".to_string(),
                    condition: None,
                },
            ],
        )];

        let replacement_defs = HashMap::new();
        let conflicts = detect_xpath_conflicts(&patches, &replacement_defs, None);

        assert_eq!(
            conflicts.len(),
            0,
            "Same-mod patches should not produce conflicts"
        );
    }

    #[test]
    fn non_overlapping_xpaths_no_conflict() {
        let mod_c = test_mod_summary_named("test.mod.c", "Mod C");
        let mod_d = test_mod_summary_named("test.mod.d", "Mod D");

        let patches = vec![
            (
                mod_c,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#.to_string(),
                    source_file: "mod_c/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
            (
                mod_d,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/costList"#.to_string(),
                    source_file: "mod_d/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
        ];

        let replacement_defs = HashMap::new();
        let conflicts = detect_xpath_conflicts(&patches, &replacement_defs, None);

        assert_eq!(
            conflicts.len(),
            0,
            "Non-overlapping xpaths should not conflict"
        );
    }

    #[test]
    fn xpath_overlap_with_load_order() {
        let mod_c = test_mod_summary_named("test.mod.c", "Mod C");
        let mod_d = test_mod_summary_named("test.mod.d", "Mod D");

        let patches = vec![
            (
                mod_c,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#.to_string(),
                    source_file: "mod_c/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
            (
                mod_d,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#.to_string(),
                    source_file: "mod_d/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
        ];

        let load_order = LoadOrder::new(vec!["test.mod.c".to_string(), "test.mod.d".to_string()]);

        let replacement_defs = HashMap::new();
        let conflicts = detect_xpath_conflicts(&patches, &replacement_defs, Some(&load_order));

        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            Conflict::Def(dc) => {
                // mod_d is later in load order
                assert_eq!(dc.winner.as_deref(), Some("test.mod.d"));
            }
            _ => panic!("Expected Conflict::Def"),
        }
    }

    #[test]
    fn wildcard_xpath_overlap_is_info() {
        let mod_c = test_mod_summary_named("test.mod.c", "Mod C");
        let mod_d = test_mod_summary_named("test.mod.d", "Mod D");

        let patches = vec![
            (
                mod_c,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: "Defs/ThingDef/statBases/MaxHitPoints".to_string(),
                    source_file: "mod_c/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
            (
                mod_d,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: r#"Defs/ThingDef[defName="Steel"]/statBases/MaxHitPoints"#.to_string(),
                    source_file: "mod_d/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
        ];

        let replacement_defs = HashMap::new();
        let conflicts = detect_xpath_conflicts(&patches, &replacement_defs, None);

        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            Conflict::Def(dc) => {
                assert!(matches!(
                    &dc.mechanism,
                    DefConflictMechanism::XPathOverlap { wildcard: true, .. }
                ));
                assert_eq!(conflicts[0].severity(), rcd_core::Severity::Info);
            }
            _ => panic!("Expected Conflict::Def"),
        }
    }

    #[test]
    fn wildcard_vs_wildcard_xpath_overlap_is_info() {
        let mod_c = test_mod_summary_named("test.mod.c", "Mod C");
        let mod_d = test_mod_summary_named("test.mod.d", "Mod D");

        let patches = vec![
            (
                mod_c,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: "Defs/ThingDef/statBases/MaxHitPoints".to_string(),
                    source_file: "mod_c/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
            (
                mod_d,
                vec![ParsedPatchOp {
                    op_type: PatchOpType::Replace,
                    xpath: "Defs/ThingDef/statBases/MaxHitPoints".to_string(),
                    source_file: "mod_d/Patches/Patches.xml".to_string(),
                    condition: None,
                }],
            ),
        ];

        let replacement_defs = HashMap::new();
        let conflicts = detect_xpath_conflicts(&patches, &replacement_defs, None);

        assert_eq!(conflicts.len(), 1);
        match &conflicts[0] {
            Conflict::Def(dc) => {
                assert!(matches!(
                    &dc.mechanism,
                    DefConflictMechanism::XPathOverlap { wildcard: true, .. }
                ));
                assert_eq!(conflicts[0].severity(), rcd_core::Severity::Info);
            }
            _ => panic!("Expected Conflict::Def"),
        }
    }
}
