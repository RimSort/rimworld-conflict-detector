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
use std::collections::{BTreeMap, HashMap};

struct XmlIssue {
    message: String,
    code: Option<String>,
    params: BTreeMap<String, String>,
}

impl XmlIssue {
    fn new(message: String, code: &str, params: &[(&str, String)]) -> Self {
        Self {
            message,
            code: Some(code.into()),
            params: params
                .iter()
                .map(|(key, value)| ((*key).into(), value.clone()))
                .collect(),
        }
    }

    fn parse_error(source: &str, detail: impl std::fmt::Display) -> Self {
        Self::new(
            format!("XML parse error in {source}: {detail}"),
            "xml_parse_failure",
            &[("path", source.into()), ("detail", detail.to_string())],
        )
    }

    fn reconstruction_error(def_id: &str, detail: impl std::fmt::Display, kind: &str) -> Self {
        let (code, prefix) = match kind {
            "text" => ("xml_reconstruction_text_failure", "text decode failed: "),
            "attribute" => (
                "xml_reconstruction_attribute_failure",
                "attribute decode failed: ",
            ),
            _ => ("xml_reconstruction_parse_failure", ""),
        };
        Self::new(
            format!("internal XML reconstruction error for {def_id}: {prefix}{detail}"),
            code,
            &[("def_id", def_id.into()), ("detail", detail.to_string())],
        )
    }
}

impl From<rcd_core::content_files::ContentWarning> for XmlIssue {
    fn from(warning: rcd_core::content_files::ContentWarning) -> Self {
        Self {
            message: warning.message,
            code: Some(warning.code.into()),
            params: warning.params,
        }
    }
}

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
                    def_parser::parse_defs_dirs_structured(&mod_data.defs_paths);
                for issue in io_errors {
                    diagnostics.push(Diagnostic {
                        affected_mod: Some(mod_summary.package_id.clone()),
                        severity: Severity::Warning,
                        message: issue.message,
                        source: DiagnosticSource::XmlParsing,
                        analysis_incomplete: true,
                        code: issue.code,
                        params: issue.params,
                    });
                }

                let (patches, patch_errors) =
                    patch_ops::parse_patches_dirs_structured(&mod_data.patches_paths);
                for issue in patch_errors {
                    diagnostics.push(Diagnostic {
                        affected_mod: Some(mod_summary.package_id.clone()),
                        severity: Severity::Warning,
                        message: issue.message,
                        source: DiagnosticSource::XmlParsing,
                        analysis_incomplete: true,
                        code: issue.code,
                        params: issue.params,
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
                        analysis_incomplete: true,
                        code: Some("xml_abstract_defs_skipped".into()),
                        params: [("count".into(), abstract_skipped.to_string())].into_iter().collect(),
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
                        analysis_incomplete: true,
                        code: Some("xml_unnamed_defs_skipped".into()),
                        params: [("count".into(), unnamed.to_string())].into_iter().collect(),
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
            replacement::detect_replacements_structured(&all_mod_defs, order);
        all_conflicts.extend(replacement_conflicts);
        for issue in recon_errors {
            all_diagnostics.push(Diagnostic {
                affected_mod: None,
                severity: Severity::Warning,
                message: issue.message,
                source: DiagnosticSource::XmlParsing,
                analysis_incomplete: true,
                code: issue.code,
                params: issue.params,
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

    #[cfg(unix)]
    #[test]
    fn skipped_defs_and_patch_symlinks_are_incomplete_diagnostics() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("About")).unwrap();
        std::fs::write(
            root.path().join("About/About.xml"),
            "<ModMetaData><packageId>test.symlink</packageId></ModMetaData>",
        )
        .unwrap();
        for folder in ["Defs", "Patches"] {
            std::fs::create_dir(root.path().join(folder)).unwrap();
            symlink("missing.xml", root.path().join(folder).join("skipped.xml")).unwrap();
        }
        let mut m = rcd_core::about_xml::parse_about_xml(root.path()).unwrap();
        m.defs_paths = vec![root.path().join("Defs")];
        m.patches_paths = vec![root.path().join("Patches")];
        let (_, diags) = XmlDetector.analyze(&[m], None);
        for folder in ["Defs", "Patches"] {
            let diagnostic = diags
                .iter()
                .find(|d| d.message.contains(folder) && d.message.contains("skipped.xml"))
                .unwrap();
            assert_eq!(diagnostic.code.as_deref(), Some("content_symlink_skipped"));
            assert!(diagnostic.params["path"].contains(folder));
            assert!(
                diags.iter().any(|d| d.analysis_incomplete
                    && d.message.contains(folder)
                    && d.message.contains("skipped.xml")),
                "{diags:?}"
            );
        }
    }

    #[test]
    fn invalid_utf8_xml_is_an_incomplete_warning() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("About")).unwrap();
        std::fs::write(
            root.path().join("About/About.xml"),
            "<ModMetaData><packageId>test.invalid</packageId></ModMetaData>",
        )
        .unwrap();
        std::fs::create_dir(root.path().join("Defs")).unwrap();
        std::fs::write(root.path().join("Defs/invalid.xml"), [0xff]).unwrap();
        let mut m = rcd_core::about_xml::parse_about_xml(root.path()).unwrap();
        m.defs_paths = vec![root.path().join("Defs")];
        let (_, diagnostics) = XmlDetector.analyze(&[m], None);
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.message.contains("invalid.xml"))
            .unwrap();
        assert_eq!(diagnostic.code.as_deref(), Some("xml_read_failure"));
        assert!(diagnostic.params["path"].contains("invalid.xml"));
        assert!(!diagnostic.params["detail"].is_empty());
        assert!(
            diagnostics.iter().any(|d| d.severity == Severity::Warning
                && d.analysis_incomplete
                && d.message.contains("invalid.xml")),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn truncated_defs_and_patches_carry_unclosed_counts() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("About")).unwrap();
        std::fs::write(
            root.path().join("About/About.xml"),
            "<ModMetaData><packageId>test.truncated</packageId></ModMetaData>",
        )
        .unwrap();
        for (folder, xml) in [
            ("Defs", "<Defs><ThingDef>"),
            ("Patches", "<Patch><Operation>"),
        ] {
            std::fs::create_dir(root.path().join(folder)).unwrap();
            std::fs::write(root.path().join(folder).join("truncated.xml"), xml).unwrap();
        }
        let mut m = rcd_core::about_xml::parse_about_xml(root.path()).unwrap();
        m.defs_paths = vec![root.path().join("Defs")];
        m.patches_paths = vec![root.path().join("Patches")];
        let (_, diagnostics) = XmlDetector.analyze(&[m], None);
        for folder in ["Defs", "Patches"] {
            let diagnostic = diagnostics
                .iter()
                .find(|d| d.message.contains(folder) && d.message.contains("unclosed element"))
                .unwrap();
            assert_eq!(diagnostic.code.as_deref(), Some("xml_unclosed_elements"));
            assert_eq!(diagnostic.params["count"], "2");
            assert!(diagnostic.params["path"].contains(folder));
            assert!(diagnostic.analysis_incomplete);
        }
    }

    #[test]
    fn nested_def_name_carries_structured_diagnostic() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("About")).unwrap();
        std::fs::write(
            root.path().join("About/About.xml"),
            "<ModMetaData><packageId>test.nested</packageId></ModMetaData>",
        )
        .unwrap();
        for (folder, xml) in [
            (
                "Defs",
                "<Defs><ThingDef><defName><b/></defName></ThingDef></Defs>",
            ),
            (
                "Patches",
                "<Patch><Operation Class=\"PatchOperationAdd\"><xpath><b/></xpath></Operation></Patch>",
            ),
        ] {
            std::fs::create_dir(root.path().join(folder)).unwrap();
            std::fs::write(root.path().join(folder).join("nested.xml"), xml).unwrap();
        }
        let mut m = rcd_core::about_xml::parse_about_xml(root.path()).unwrap();
        m.defs_paths = vec![root.path().join("Defs")];
        m.patches_paths = vec![root.path().join("Patches")];
        let (_, diagnostics) = XmlDetector.analyze(&[m], None);
        let def = diagnostics
            .iter()
            .find(|d| d.message.contains("defName") && d.message.contains("nested markup"))
            .unwrap();
        assert_eq!(def.code.as_deref(), Some("xml_def_name_nested"));
        assert!(def.params["path"].contains("Defs"));
    }

    #[test]
    fn nested_patch_xpath_carries_structured_diagnostic() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("nested.xml");
        std::fs::write(
            &path,
            "<Patch><Operation Class=\"PatchOperationAdd\"><xpath><b/></xpath></Operation></Patch>",
        )
        .unwrap();
        let (_, issues) = patch_ops::parse_patches_dirs_structured(&[root.path().into()]);
        let issue = issues
            .iter()
            .find(|d| d.message.contains("Nested element"))
            .unwrap();
        assert_eq!(issue.code.as_deref(), Some("xml_text_nested"));
        assert_eq!(issue.params["element"], "xpath");
        assert_eq!(issue.params["path"], path.to_string_lossy());
    }

    #[test]
    fn reconstruction_failure_reaches_detector_diagnostics() {
        let root = tempfile::tempdir().unwrap();
        let mut mods = Vec::new();
        for name in ["a", "b"] {
            let path = root.path().join(name);
            std::fs::create_dir_all(path.join("About")).unwrap();
            std::fs::create_dir_all(path.join("Defs")).unwrap();
            std::fs::write(
                path.join("About/About.xml"),
                format!("<ModMetaData><packageId>test.{name}</packageId></ModMetaData>"),
            )
            .unwrap();
            std::fs::write(
                path.join("Defs/a.xml"),
                "<Defs><ThingDef><defName>Steel</defName><label>&bogus;</label></ThingDef></Defs>",
            )
            .unwrap();
            let mut m = rcd_core::about_xml::parse_about_xml(&path).unwrap();
            m.defs_paths = vec![path.join("Defs")];
            mods.push(m);
        }
        let (_, diagnostics) = XmlDetector.analyze(&mods, None);
        let diagnostic = diagnostics
            .iter()
            .find(|d| d.code.as_deref() == Some("xml_reconstruction_text_failure"))
            .unwrap();
        assert_eq!(diagnostic.params["def_id"], "ThingDef:Steel");
        assert!(!diagnostic.params["detail"].is_empty());
        assert_eq!(diagnostic.severity, Severity::Warning);
        assert!(diagnostic.analysis_incomplete);
    }

    #[test]
    fn failed_xml_analysis_is_incomplete() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("About")).unwrap();
        std::fs::write(
            root.path().join("About/About.xml"),
            "<ModMetaData><packageId>test.broken</packageId></ModMetaData>",
        )
        .unwrap();
        std::fs::create_dir(root.path().join("Defs")).unwrap();
        std::fs::write(root.path().join("Defs/broken.xml"), "<Defs><ThingDef>").unwrap();
        let mut m = rcd_core::about_xml::parse_about_xml(root.path()).unwrap();
        m.defs_paths = vec![root.path().join("Defs")];
        let (_, diags) = XmlDetector.analyze(&[m], None);
        assert!(
            diags
                .iter()
                .any(|d| d.analysis_incomplete && d.source == DiagnosticSource::XmlParsing),
            "{diags:?}"
        );
    }

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
        assert_eq!(
            abstract_diags[0].code.as_deref(),
            Some("xml_abstract_defs_skipped")
        );
        assert_eq!(abstract_diags[0].params["count"], "1");
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
        assert_eq!(
            unnamed_diags[0].code.as_deref(),
            Some("xml_unnamed_defs_skipped")
        );
        assert_eq!(unnamed_diags[0].params["count"], "2");
        assert_eq!(unnamed_diags[0].severity, Severity::Info);
        assert!(unnamed_diags[0].analysis_incomplete);
        assert!(unnamed_diags[0].message.contains("2 def(s)"));
        assert_eq!(
            unnamed_diags[0].affected_mod.as_deref(),
            Some("test.unnamed")
        );
    }
}
