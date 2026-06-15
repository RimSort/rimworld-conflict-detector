use std::collections::HashMap;

use rcd_core::{
    Conflict, HarmonyConflict, HarmonyPatchInfo, HarmonyPatchKind, HarmonyTarget, LoadOrder,
    ModSummary,
};

use crate::attributes::DiscoveredPatch;

/// Key for grouping patches by target method.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct PatchTarget {
    target_type: String,
    target_method: String,
}

/// Detect Harmony patch conflicts when multiple mods patch the same method.
///
/// Builds a map of `(target_type, target_method)` to all patches targeting that method,
/// then emits a `Conflict::Harmony` for each method patched by 2+ different mods.
pub fn detect_harmony_conflicts(
    all_patches: &[(ModSummary, Vec<DiscoveredPatch>)],
    order: Option<&LoadOrder>,
) -> Vec<Conflict> {
    // Step 1: Build a map: (target_type, target_method) -> Vec<(ModSummary, DiscoveredPatch)>
    let mut target_map: HashMap<PatchTarget, Vec<(&ModSummary, &DiscoveredPatch)>> = HashMap::new();

    for (mod_summary, patches) in all_patches {
        for patch in patches {
            // Skip entries where target_type or target_method is None
            let (Some(target_type), Some(target_method)) =
                (patch.target_type.as_ref(), patch.target_method.as_ref())
            else {
                continue;
            };

            let key = PatchTarget {
                target_type: target_type.clone(),
                target_method: target_method.clone(),
            };

            target_map
                .entry(key)
                .or_default()
                .push((mod_summary, patch));
        }
    }

    // Step 2: For keys with 2+ entries from DIFFERENT mods, create a Conflict
    let mut conflicts = Vec::new();

    for (target, entries) in &target_map {
        // Check that at least 2 different mods are involved
        let distinct_mods: Vec<&str> = {
            let mut ids: Vec<&str> = entries.iter().map(|(m, _)| m.package_id.as_str()).collect();
            ids.sort_unstable();
            ids.dedup();
            ids
        };

        if distinct_mods.len() < 2 {
            continue;
        }

        // Step 3: Determine short_circuit_risk
        // True if there are multiple patches and any is a Prefix that returns_bool
        let has_multiple_prefixes = entries
            .iter()
            .filter(|(_, p)| p.patch_kind == HarmonyPatchKind::Prefix)
            .count()
            >= 2;
        let any_prefix_returns_bool = entries
            .iter()
            .any(|(_, p)| p.patch_kind == HarmonyPatchKind::Prefix && p.returns_bool);
        let short_circuit_risk = has_multiple_prefixes && any_prefix_returns_bool;

        // Step 5: Build execution_order from priority values when load order is available
        let execution_order = build_execution_order(entries, order);

        // Step 6: Map each DiscoveredPatch to a HarmonyPatchInfo
        let patches: Vec<HarmonyPatchInfo> = entries
            .iter()
            .map(|(mod_summary, patch)| to_patch_info(mod_summary, patch))
            .collect();

        // Determine the most common parameter_types across patches for this target
        let parameter_types = entries.iter().find_map(|(_, p)| p.parameter_types.clone());

        conflicts.push(Conflict::Harmony(HarmonyConflict {
            target: HarmonyTarget {
                type_name: target.target_type.clone(),
                method_name: target.target_method.clone(),
                parameter_types,
            },
            patches,
            short_circuit_risk,
            execution_order,
        }));
    }

    // Sort conflicts for deterministic output
    conflicts.sort_by(|a, b| {
        let Conflict::Harmony(a) = a else {
            unreachable!()
        };
        let Conflict::Harmony(b) = b else {
            unreachable!()
        };
        a.target
            .type_name
            .cmp(&b.target.type_name)
            .then_with(|| a.target.method_name.cmp(&b.target.method_name))
    });

    conflicts
}

/// Build an execution order list based on priority values, Before/After constraints,
/// and load order as tiebreaker.
///
/// 1. Sort by priority (higher runs first)
/// 2. Within each priority tier, apply [HarmonyBefore]/[HarmonyAfter] constraints
///    via topological sort (petgraph)
/// 3. Load order is the tiebreaker for unconstrained pairs within the same priority
fn build_execution_order(
    entries: &[(&ModSummary, &DiscoveredPatch)],
    order: Option<&LoadOrder>,
) -> Option<Vec<String>> {
    let order = order?;

    // Group entries by priority
    let mut by_priority: HashMap<i32, Vec<(&ModSummary, &DiscoveredPatch)>> = HashMap::new();
    for entry in entries {
        by_priority
            .entry(entry.1.priority)
            .or_default()
            .push(*entry);
    }

    // Sort priority tiers descending (higher priority runs first)
    let mut priority_tiers: Vec<i32> = by_priority.keys().copied().collect();
    priority_tiers.sort_unstable_by(|a, b| b.cmp(a));

    let mut result = Vec::new();

    for priority in priority_tiers {
        let tier = &by_priority[&priority];

        if tier.len() <= 1 {
            if let Some((ms, _)) = tier.first() {
                result.push(ms.package_id.clone());
            }
            continue;
        }

        // Build a DAG for this tier using Before/After constraints
        use petgraph::algo::toposort;
        use petgraph::graph::DiGraph;

        let mut graph = DiGraph::<usize, ()>::new();
        let mut idx_to_node: Vec<petgraph::graph::NodeIndex> = Vec::new();

        // Add nodes (one per entry in this tier)
        for i in 0..tier.len() {
            idx_to_node.push(graph.add_node(i));
        }

        // Add edges from Before/After constraints
        for (i, (_ms_i, patch_i)) in tier.iter().enumerate() {
            // If patch_i declares HarmonyBefore("mod.x"), i should come before mod.x
            for before_id in &patch_i.before {
                if let Some(j) = tier
                    .iter()
                    .position(|(ms_j, _)| ms_j.package_id == *before_id)
                {
                    graph.add_edge(idx_to_node[i], idx_to_node[j], ());
                }
            }
            // If patch_i declares HarmonyAfter("mod.x"), mod.x should come before i
            for after_id in &patch_i.after {
                if let Some(j) = tier
                    .iter()
                    .position(|(ms_j, _)| ms_j.package_id == *after_id)
                {
                    graph.add_edge(idx_to_node[j], idx_to_node[i], ());
                }
            }
        }

        // Topological sort; fall back to load order on cycle
        match toposort(&graph, None) {
            Ok(sorted_nodes) => {
                // Within unconstrained groups, sort by load order
                let sorted_ids: Vec<String> = sorted_nodes
                    .iter()
                    .map(|node| {
                        let idx = graph[*node];
                        tier[idx].0.package_id.clone()
                    })
                    .collect();
                result.extend(sorted_ids);
            }
            Err(_cycle) => {
                // Cycle detected — fall back to load order
                let mut fallback: Vec<(&ModSummary, &DiscoveredPatch)> = tier.clone();
                fallback
                    .sort_by_key(|(ms, _)| order.position(&ms.package_id).unwrap_or(usize::MAX));
                result.extend(fallback.iter().map(|(ms, _)| ms.package_id.clone()));
            }
        }
    }

    Some(result)
}

/// Convert a DiscoveredPatch into a HarmonyPatchInfo for the conflict model.
fn to_patch_info(mod_summary: &ModSummary, patch: &DiscoveredPatch) -> HarmonyPatchInfo {
    HarmonyPatchInfo {
        mod_summary: mod_summary.clone(),
        dll_name: patch.dll_name.clone(),
        patch_kind: patch.patch_kind,
        priority: patch.priority,
        before: patch.before.clone(),
        after: patch.after.clone(),
        returns_bool: patch.returns_bool,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcd_core::Severity;

    fn test_mod(id: &str) -> ModSummary {
        ModSummary {
            package_id: id.to_string(),
            name: id.to_string(),
            author: None,
            is_base_content: false,
        }
    }

    fn make_patch(
        target_type: &str,
        target_method: &str,
        kind: HarmonyPatchKind,
        returns_bool: bool,
        priority: i32,
    ) -> DiscoveredPatch {
        DiscoveredPatch {
            target_type: Some(target_type.to_string()),
            target_method: Some(target_method.to_string()),
            parameter_types: None,
            patch_kind: kind,
            priority,
            before: vec![],
            after: vec![],
            returns_bool,
            dll_name: "Test.dll".to_string(),
        }
    }

    #[test]
    fn two_prefixes_with_bool_is_error() {
        let all_patches = vec![
            (
                test_mod("mod.a"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    true,
                    400,
                )],
            ),
            (
                test_mod("mod.b"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    true,
                    400,
                )],
            ),
        ];

        let conflicts = detect_harmony_conflicts(&all_patches, None);
        assert_eq!(conflicts.len(), 1);

        let conflict = &conflicts[0];
        assert_eq!(conflict.severity(), Severity::Error);

        if let Conflict::Harmony(hc) = conflict {
            assert!(hc.short_circuit_risk);
            assert_eq!(hc.patches.len(), 2);
            assert_eq!(hc.target.type_name, "Pawn");
            assert_eq!(hc.target.method_name, "Kill");
        } else {
            panic!("Expected Harmony conflict");
        }
    }

    #[test]
    fn two_postfixes_is_info() {
        let all_patches = vec![
            (
                test_mod("mod.a"),
                vec![make_patch(
                    "Pawn",
                    "GetGizmos",
                    HarmonyPatchKind::Postfix,
                    false,
                    400,
                )],
            ),
            (
                test_mod("mod.b"),
                vec![make_patch(
                    "Pawn",
                    "GetGizmos",
                    HarmonyPatchKind::Postfix,
                    false,
                    400,
                )],
            ),
        ];

        let conflicts = detect_harmony_conflicts(&all_patches, None);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].severity(), Severity::Info);

        if let Conflict::Harmony(hc) = &conflicts[0] {
            assert!(!hc.short_circuit_risk);
        } else {
            panic!("Expected Harmony conflict");
        }
    }

    #[test]
    fn prefix_plus_transpiler_is_warning() {
        let all_patches = vec![
            (
                test_mod("mod.a"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    400,
                )],
            ),
            (
                test_mod("mod.b"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Transpiler,
                    false,
                    400,
                )],
            ),
        ];

        let conflicts = detect_harmony_conflicts(&all_patches, None);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].severity(), Severity::Warning);

        if let Conflict::Harmony(hc) = &conflicts[0] {
            assert!(!hc.short_circuit_risk);
        } else {
            panic!("Expected Harmony conflict");
        }
    }

    #[test]
    fn same_mod_not_a_conflict() {
        let all_patches = vec![(
            test_mod("mod.a"),
            vec![
                make_patch("Pawn", "Kill", HarmonyPatchKind::Prefix, true, 400),
                make_patch("Pawn", "Kill", HarmonyPatchKind::Postfix, false, 400),
            ],
        )];

        let conflicts = detect_harmony_conflicts(&all_patches, None);
        assert!(
            conflicts.is_empty(),
            "Patches from the same mod should not produce a conflict"
        );
    }

    #[test]
    fn execution_order_from_priority() {
        let order = LoadOrder {
            package_ids: vec![
                "mod.low".to_string(),
                "mod.high".to_string(),
                "mod.mid".to_string(),
            ],
        };

        let all_patches = vec![
            (
                test_mod("mod.low"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    200, // low priority
                )],
            ),
            (
                test_mod("mod.high"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    600, // high priority
                )],
            ),
            (
                test_mod("mod.mid"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    400, // normal priority
                )],
            ),
        ];

        let conflicts = detect_harmony_conflicts(&all_patches, Some(&order));
        assert_eq!(conflicts.len(), 1);

        if let Conflict::Harmony(hc) = &conflicts[0] {
            let exec_order = hc
                .execution_order
                .as_ref()
                .expect("should have execution_order");
            // Higher priority runs first
            assert_eq!(
                exec_order[0], "mod.high",
                "highest priority should be first"
            );
            assert_eq!(exec_order[1], "mod.mid", "normal priority should be second");
            assert_eq!(exec_order[2], "mod.low", "lowest priority should be last");
        } else {
            panic!("Expected Harmony conflict");
        }
    }

    #[test]
    fn execution_order_tiebreak_by_load_order() {
        let order = LoadOrder {
            package_ids: vec!["mod.first".to_string(), "mod.second".to_string()],
        };

        let all_patches = vec![
            (
                test_mod("mod.second"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    400, // same priority
                )],
            ),
            (
                test_mod("mod.first"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    400, // same priority
                )],
            ),
        ];

        let conflicts = detect_harmony_conflicts(&all_patches, Some(&order));
        assert_eq!(conflicts.len(), 1);

        if let Conflict::Harmony(hc) = &conflicts[0] {
            let exec_order = hc
                .execution_order
                .as_ref()
                .expect("should have execution_order");
            // Same priority, so load order is tiebreaker (earlier first)
            assert_eq!(exec_order[0], "mod.first");
            assert_eq!(exec_order[1], "mod.second");
        } else {
            panic!("Expected Harmony conflict");
        }
    }

    #[test]
    fn no_execution_order_without_load_order() {
        let all_patches = vec![
            (
                test_mod("mod.a"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    400,
                )],
            ),
            (
                test_mod("mod.b"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    400,
                )],
            ),
        ];

        let conflicts = detect_harmony_conflicts(&all_patches, None);
        assert_eq!(conflicts.len(), 1);

        if let Conflict::Harmony(hc) = &conflicts[0] {
            assert!(
                hc.execution_order.is_none(),
                "execution_order should be None when no load order provided"
            );
        } else {
            panic!("Expected Harmony conflict");
        }
    }

    #[test]
    fn skips_patches_without_target() {
        let all_patches = vec![
            (
                test_mod("mod.a"),
                vec![DiscoveredPatch {
                    target_type: None,
                    target_method: Some("Kill".to_string()),
                    parameter_types: None,
                    patch_kind: HarmonyPatchKind::Prefix,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: true,
                    dll_name: "Test.dll".to_string(),
                }],
            ),
            (
                test_mod("mod.b"),
                vec![DiscoveredPatch {
                    target_type: Some("Pawn".to_string()),
                    target_method: None,
                    parameter_types: None,
                    patch_kind: HarmonyPatchKind::Prefix,
                    priority: 400,
                    before: vec![],
                    after: vec![],
                    returns_bool: true,
                    dll_name: "Test.dll".to_string(),
                }],
            ),
        ];

        let conflicts = detect_harmony_conflicts(&all_patches, None);
        assert!(
            conflicts.is_empty(),
            "Patches with missing target_type or target_method should be skipped"
        );
    }

    #[test]
    fn before_after_constraints_override_load_order() {
        let order = LoadOrder {
            package_ids: vec!["mod.a".to_string(), "mod.b".to_string()],
        };

        // mod.a loads first but declares HarmonyAfter("mod.b"),
        // so mod.b should come before mod.a in execution order
        let all_patches = vec![
            (
                test_mod("mod.a"),
                vec![{
                    let mut p = make_patch("Pawn", "Kill", HarmonyPatchKind::Prefix, false, 400);
                    p.after = vec!["mod.b".to_string()];
                    p
                }],
            ),
            (
                test_mod("mod.b"),
                vec![make_patch(
                    "Pawn",
                    "Kill",
                    HarmonyPatchKind::Prefix,
                    false,
                    400,
                )],
            ),
        ];

        let conflicts = detect_harmony_conflicts(&all_patches, Some(&order));
        assert_eq!(conflicts.len(), 1);

        if let Conflict::Harmony(hc) = &conflicts[0] {
            let exec_order = hc
                .execution_order
                .as_ref()
                .expect("should have execution_order");
            assert_eq!(
                exec_order[0], "mod.b",
                "mod.b should run first (mod.a declares HarmonyAfter mod.b)"
            );
            assert_eq!(exec_order[1], "mod.a");
        } else {
            panic!("Expected Harmony conflict");
        }
    }
}
