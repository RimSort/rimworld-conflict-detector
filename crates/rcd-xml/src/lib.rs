// RimWorld Conflict Detector - XML analysis

pub mod conditional;
pub mod conflict;
pub mod def_parser;
pub mod patch_ops;
pub mod replacement;
pub mod xpath;

use rayon::prelude::*;
use rcd_core::{
    Conflict, Detector, Diagnostic, DiagnosticSource, LoadOrder, Mod, ModSummary, Severity,
};
use std::collections::HashMap;

/// The XML detector for RimWorld mods.
///
/// Analyzes XML defs and patch operations to detect:
/// - Direct replacement conflicts (multiple mods define the same def)
/// - XPath overlap conflicts (multiple mods patch the same XPath)
/// - XPath on replaced def conflicts (a mod patches a def that another mod replaces)
pub struct XmlDetector;

impl Detector for XmlDetector {
    fn name(&self) -> &str {
        "xml"
    }

    fn analyze(&self, mods: &[Mod], order: Option<&LoadOrder>) -> (Vec<Conflict>, Vec<Diagnostic>) {
        let mut all_conflicts = Vec::new();

        // Parallel parsing of mods — each mod's XML parsing is independent
        let parsed: Vec<_> = mods
            .par_iter()
            .map(|mod_data| {
                let mod_summary = ModSummary::from(mod_data);
                let mut diagnostics = Vec::new();

                // Parse all roots together so first-wins file precedence spans
                // the complete LoadFolders priority list.
                let (defs, abstract_skipped, unnamed, io_errors) =
                    def_parser::parse_defs_dirs(&mod_data.defs_paths);
                for err_msg in io_errors {
                    diagnostics.push(Diagnostic {
                        affected_mod: Some(mod_summary.package_id.clone()),
                        severity: Severity::Warning,
                        message: err_msg,
                        source: DiagnosticSource::XmlParsing,
                    });
                }

                let (patches, patch_errors) =
                    patch_ops::parse_patches_dirs(&mod_data.patches_paths);
                for err_msg in patch_errors {
                    diagnostics.push(Diagnostic {
                        affected_mod: Some(mod_summary.package_id.clone()),
                        severity: Severity::Warning,
                        message: err_msg,
                        source: DiagnosticSource::XmlParsing,
                    });
                }

                if abstract_skipped > 0 {
                    diagnostics.push(Diagnostic {
                        affected_mod: Some(mod_summary.package_id.clone()),
                        severity: Severity::Info,
                        message: format!(
                            "Skipped {} abstract def(s) without a usable defName",
                            abstract_skipped
                        ),
                        source: DiagnosticSource::XmlParsing,
                    });
                }
                if unnamed > 0 {
                    diagnostics.push(Diagnostic {
                        affected_mod: Some(mod_summary.package_id.clone()),
                        severity: Severity::Info,
                        message: format!(
                            "Skipped {} def(s) without defName (unindexable; RimWorld auto-names these at runtime)",
                            unnamed
                        ),
                        source: DiagnosticSource::XmlParsing,
                    });
                }
                (mod_summary, defs, patches, diagnostics)
            })
            .collect();

        // Build mod_defs and mod_patches lists from parsed results
        let mut all_mod_defs = Vec::new();
        let mut all_mod_patches = Vec::new();
        let mut all_diagnostics = Vec::new();

        for (mod_summary, defs, patches, diagnostics) in parsed {
            all_mod_defs.push(replacement::ModDefs {
                mod_summary: mod_summary.clone(),
                defs,
            });

            if !patches.is_empty() {
                all_mod_patches.push((mod_summary, patches));
            }

            all_diagnostics.extend(diagnostics);
        }

        // 3. Detect direct replacement conflicts
        let (replacement_conflicts, recon_errors) =
            replacement::detect_replacements(&all_mod_defs, order);
        all_conflicts.extend(replacement_conflicts);
        for msg in recon_errors {
            all_diagnostics.push(Diagnostic {
                affected_mod: None,
                severity: Severity::Warning,
                message: msg,
                source: DiagnosticSource::XmlParsing,
            });
        }

        // 4. Build replacement map for XPath conflict detection
        let replacement_map = build_replacement_map(&all_mod_defs);

        // 5. Detect XPath conflicts
        let xpath_conflicts =
            conflict::detect_xpath_conflicts(&all_mod_patches, &replacement_map, order);
        all_conflicts.extend(xpath_conflicts);

        (all_conflicts, all_diagnostics)
    }
}

/// Build a map of (def_type, def_name) to the mods that define it.
/// Only includes defs that appear in 2+ mods (potential replacements).
fn build_replacement_map(
    all_mod_defs: &[replacement::ModDefs],
) -> HashMap<(String, String), Vec<ModSummary>> {
    let mut map: HashMap<(String, String), Vec<ModSummary>> = HashMap::new();

    for mod_defs in all_mod_defs {
        for def in &mod_defs.defs {
            let key = (def.def_type.clone(), def.def_name.clone());
            let entry = map.entry(key).or_default();
            if !entry
                .iter()
                .any(|ms| ms.package_id == mod_defs.mod_summary.package_id)
            {
                entry.push(mod_defs.mod_summary.clone());
            }
        }
    }

    // Only keep entries with 2+ distinct mods
    map.retain(|_, mods| mods.len() >= 2);
    map
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use rcd_core::folder::discover_mods;
    use rcd_core::testutil::fixtures_path;

    #[test]
    fn xml_detector_finds_steel_conflict() {
        let (mods, _diagnostics) = discover_mods(&[fixtures_path()], Some("1.5"));
        let detector = XmlDetector;
        let (conflicts, _diagnostics) = detector.analyze(&mods, None);

        // Should find at least one Error conflict (mod_a and mod_b both define ThingDef:Steel)
        let errors: Vec<_> = conflicts
            .iter()
            .filter(|c| c.severity() == Severity::Error)
            .collect();
        assert!(
            !errors.is_empty(),
            "Should detect ThingDef:Steel replacement conflict"
        );
    }

    #[test]
    fn detector_applies_defs_overlay_across_all_mod_paths() {
        use rcd_core::Mod;

        let temp = tempfile::tempdir().unwrap();
        let high = temp.path().join("high");
        let low = temp.path().join("low");
        let peer = temp.path().join("peer");
        let high_patches = temp.path().join("high_patches");
        let low_patches = temp.path().join("low_patches");
        let peer_patches = temp.path().join("peer_patches");
        for root in [
            &high,
            &low,
            &peer,
            &high_patches,
            &low_patches,
            &peer_patches,
        ] {
            std::fs::create_dir_all(root).unwrap();
        }
        std::fs::write(
            high.join("same.xml"),
            "<Defs><ThingDef><defName>High</defName><label>high</label></ThingDef></Defs>",
        )
        .unwrap();
        std::fs::write(
            low.join("same.xml"),
            "<Defs><ThingDef><defName>Low</defName><label>low</label></ThingDef></Defs>",
        )
        .unwrap();
        std::fs::write(
            low.join("unique.xml"),
            "<Defs><ThingDef><defName>Unique</defName><label>low unique</label></ThingDef></Defs>",
        )
        .unwrap();
        std::fs::write(
            peer.join("peer.xml"),
            "<Defs><ThingDef><defName>Low</defName><label>peer</label></ThingDef><ThingDef><defName>Unique</defName><label>peer</label></ThingDef></Defs>",
        )
        .unwrap();
        std::fs::write(
            high_patches.join("same.xml"),
            r#"<Patch><Operation Class="PatchOperationReplace"><xpath>Defs/ThingDef[defName="HighPatch"]</xpath></Operation></Patch>"#,
        )
        .unwrap();
        std::fs::write(
            low_patches.join("same.xml"),
            r#"<Patch><Operation Class="PatchOperationReplace"><xpath>Defs/ThingDef[defName="LowPatch"]</xpath></Operation></Patch>"#,
        )
        .unwrap();
        std::fs::create_dir_all(low_patches.join("nested")).unwrap();
        std::fs::write(
            low_patches.join("nested/unique.xml"),
            r#"<Patch><Operation Class="PatchOperationReplace"><xpath>Defs/ThingDef[defName="UniquePatch"]</xpath></Operation></Patch>"#,
        )
        .unwrap();
        std::fs::write(
            peer_patches.join("peer.xml"),
            r#"<Patch><Operation Class="PatchOperationReplace"><xpath>Defs/ThingDef[defName="LowPatch"]</xpath></Operation><Operation Class="PatchOperationReplace"><xpath>Defs/ThingDef[defName="UniquePatch"]</xpath></Operation></Patch>"#,
        )
        .unwrap();

        let make_mod = |package_id: &str,
                        name: &str,
                        paths: Vec<std::path::PathBuf>,
                        patches: Vec<std::path::PathBuf>| Mod {
            package_id: package_id.to_string(),
            name: name.to_string(),
            author: None,
            is_base_content: false,
            root_path: temp.path().to_path_buf(),
            supported_versions: vec![],
            dependencies: vec![],
            incompatibilities: vec![],
            load_before: vec![],
            load_after: vec![],
            force_load_before: vec![],
            force_load_after: vec![],
            defs_paths: paths,
            patches_paths: patches,
            assemblies_paths: vec![],
        };
        let mods = vec![
            make_mod(
                "test.overlay",
                "Overlay",
                vec![high, low],
                vec![high_patches, low_patches],
            ),
            make_mod("test.peer", "Peer", vec![peer], vec![peer_patches]),
        ];

        let (conflicts, diagnostics) = XmlDetector.analyze(&mods, None);
        assert!(
            diagnostics.is_empty(),
            "unexpected diagnostics: {diagnostics:?}"
        );
        let targets: Vec<_> = conflicts
            .iter()
            .filter_map(|conflict| match conflict {
                Conflict::Def(def)
                    if matches!(
                        def.mechanism,
                        rcd_core::DefConflictMechanism::DirectReplacement { .. }
                    ) =>
                {
                    Some((def.target.def_type.clone(), def.target.def_name.clone()))
                }
                Conflict::Def(_) => None,
                _ => None,
            })
            .collect();
        assert_eq!(
            targets,
            vec![("ThingDef".to_string(), "Unique".to_string())],
            "only the unique lower-priority def file should remain visible"
        );
        let xpath_targets: Vec<_> = conflicts
            .iter()
            .filter_map(|conflict| match conflict {
                Conflict::Def(def)
                    if matches!(
                        def.mechanism,
                        rcd_core::DefConflictMechanism::XPathOverlap { .. }
                    ) =>
                {
                    Some(def.target.def_name.clone())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            xpath_targets,
            vec!["UniquePatch"],
            "duplicate lower-priority patch must be excluded while its unique nested file remains visible"
        );
    }

    #[test]
    fn same_mod_version_folders_not_in_replacement_map() {
        let mod_defs = vec![replacement::ModDefs {
            mod_summary: ModSummary {
                package_id: "test.mod.versioned".to_string(),
                name: "Versioned Mod".to_string(),
                author: None,
                is_base_content: false,
            },
            defs: vec![
                def_parser::ParsedDef {
                    def_type: "ThingDef".to_string(),
                    def_name: "VersionedItem".to_string(),
                    raw_xml: "<ThingDef><defName>VersionedItem</defName></ThingDef>".to_string(),
                    source_file: "1.5/Defs/ThingDefs.xml".to_string(),
                },
                def_parser::ParsedDef {
                    def_type: "ThingDef".to_string(),
                    def_name: "VersionedItem".to_string(),
                    raw_xml:
                        "<ThingDef><defName>VersionedItem</defName><label>legacy</label></ThingDef>"
                            .to_string(),
                    source_file: "Common/Defs/ThingDefs.xml".to_string(),
                },
            ],
        }];

        let map = build_replacement_map(&mod_defs);
        assert!(
            map.is_empty(),
            "Same mod providing a def from multiple folders should not appear in replacement map"
        );
    }

    #[test]
    fn versioned_mod_no_self_conflict() {
        let (mods, _diagnostics) = discover_mods(&[fixtures_path()], Some("1.5"));
        let versioned = mods.iter().find(|m| m.package_id == "test.mod.versioned");
        assert!(versioned.is_some(), "Should find versioned fixture mod");

        let detector = XmlDetector;
        let (conflicts, _) = detector.analyze(&mods, None);

        let self_conflicts: Vec<_> = conflicts
            .iter()
            .filter(|c| {
                let involved = c.involved_mods();
                involved.len() >= 2
                    && involved
                        .iter()
                        .all(|m| m.package_id == "test.mod.versioned")
            })
            .collect();

        assert_eq!(
            self_conflicts.len(),
            0,
            "Versioned mod should not conflict with itself: {:?}",
            self_conflicts
        );
    }

    #[test]
    fn abstract_defs_emit_info_diagnostic() {
        let xml_defs = r#"<?xml version="1.0" encoding="utf-8"?>
<Defs>
    <ThingDef Abstract="True">
        <label>base thing</label>
    </ThingDef>
    <ThingDef>
        <defName>Steel</defName>
    </ThingDef>
</Defs>"#;

        let dir = tempfile::tempdir().unwrap();
        let defs_dir = dir.path().join("Defs");
        std::fs::create_dir_all(&defs_dir).unwrap();
        std::fs::write(defs_dir.join("ThingDefs.xml"), xml_defs).unwrap();

        let m = rcd_core::Mod {
            package_id: "test.abstract".to_string(),
            name: "Abstract Test".to_string(),
            author: None,
            is_base_content: false,
            root_path: dir.path().to_path_buf(),
            supported_versions: vec![],
            dependencies: vec![],
            incompatibilities: vec![],
            load_before: vec![],
            load_after: vec![],
            force_load_before: vec![],
            force_load_after: vec![],
            defs_paths: vec![defs_dir],
            patches_paths: vec![],
            assemblies_paths: vec![],
        };

        let detector = XmlDetector;
        let (_conflicts, diagnostics) = detector.analyze(&[m], None);

        let abstract_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.message.contains("abstract def"))
            .collect();

        assert_eq!(abstract_diags.len(), 1);
        assert_eq!(abstract_diags[0].severity, Severity::Info);
        assert!(abstract_diags[0].message.contains("1 abstract def"));
        assert_eq!(
            abstract_diags[0].affected_mod.as_deref(),
            Some("test.abstract")
        );
    }

    #[test]
    fn unnamed_concrete_defs_emit_one_aggregated_info_diagnostic() {
        // Vanilla Core/Royalty ship SongDefs without defName; a corpus run
        // proved per-def Warnings on them are noise. The drop stays, the
        // report aggregates to one Info per mod.
        let xml_defs = r#"<?xml version="1.0" encoding="utf-8"?>
<Defs>
    <SongDef>
        <clipPath>Songs/x</clipPath>
    </SongDef>
    <SongDef>
        <clipPath>Songs/y</clipPath>
    </SongDef>
    <ThingDef>
        <defName>Steel</defName>
    </ThingDef>
</Defs>"#;

        let dir = tempfile::tempdir().unwrap();
        let defs_dir = dir.path().join("Defs");
        std::fs::create_dir_all(&defs_dir).unwrap();
        std::fs::write(defs_dir.join("Songs.xml"), xml_defs).unwrap();

        let m = rcd_core::Mod {
            package_id: "test.unnamed".to_string(),
            name: "Unnamed Test".to_string(),
            author: None,
            is_base_content: false,
            root_path: dir.path().to_path_buf(),
            supported_versions: vec![],
            dependencies: vec![],
            incompatibilities: vec![],
            load_before: vec![],
            load_after: vec![],
            force_load_before: vec![],
            force_load_after: vec![],
            defs_paths: vec![defs_dir],
            patches_paths: vec![],
            assemblies_paths: vec![],
        };

        let detector = XmlDetector;
        let (_conflicts, diagnostics) = detector.analyze(&[m], None);

        let unnamed_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.message.contains("without defName (unindexable"))
            .collect();

        assert_eq!(
            unnamed_diags.len(),
            1,
            "one aggregated Info per mod: {diagnostics:?}"
        );
        assert_eq!(unnamed_diags[0].severity, Severity::Info);
        assert!(unnamed_diags[0].message.contains("2 def(s)"));
        assert_eq!(
            unnamed_diags[0].affected_mod.as_deref(),
            Some("test.unnamed")
        );
    }
}
