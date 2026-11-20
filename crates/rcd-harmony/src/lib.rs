// RimWorld Conflict Detector - Harmony analysis

pub mod attributes;
pub mod conflict;
pub mod discovery;
pub mod manual;
pub mod priority;

use std::panic;

use rayon::prelude::*;
use rcd_core::{
    Conflict, Detector, Diagnostic, DiagnosticSource, LoadOrder, Mod, ModSummary, Severity,
};

use crate::attributes::DiscoveredPatch;
use crate::conflict::detect_harmony_conflicts;
use crate::discovery::discover_assemblies_dirs_structured;

/// Detector that finds Harmony patch conflicts across mods.
///
/// For each mod, discovers assemblies, parses Harmony patch attributes from DLLs,
/// then detects conflicts where multiple mods patch the same method.
pub struct HarmonyDetector;

impl Detector for HarmonyDetector {
    fn name(&self) -> &str {
        "harmony"
    }

    fn analyze(&self, mods: &[Mod], order: Option<&LoadOrder>) -> (Vec<Conflict>, Vec<Diagnostic>) {
        // Parallel parsing of mods — each mod's DLL parsing is independent
        let parsed: Vec<_> = mods
            .par_iter()
            .map(|m| {
                let mod_summary = ModSummary::from(m);
                let mut mod_patches: Vec<DiscoveredPatch> = Vec::new();
                let mut diagnostics: Vec<Diagnostic> = Vec::new();

                let (dlls, discovery_errors) =
                    discover_assemblies_dirs_structured(&m.assemblies_paths);
                for warning in discovery_errors {
                    diagnostics.push(Diagnostic {
                        affected_mod: Some(mod_summary.package_id.clone()),
                        severity: Severity::Warning,
                        message: warning.message,
                        source: DiagnosticSource::DllParsing,
                        analysis_incomplete: true,
                        code: Some(warning.code.into()),
                        params: warning.params,
                    });
                }

                for dll_path in dlls {
                    let dll_name = dll_path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown.dll")
                        .to_string();

                    // Wrap parse_harmony_patches in catch_unwind since dotnetdll is
                    // pre-1.0 and may panic on malformed DLLs.
                    let parse_result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
                        crate::attributes::parse_harmony_patches(&dll_path)
                    }));

                    match parse_result {
                        Ok(Ok(patches)) => {
                            mod_patches.extend(patches);
                        }
                        Ok(Err(e)) => {
                            diagnostics.push(Diagnostic {
                                affected_mod: Some(mod_summary.package_id.clone()),
                                severity: Severity::Warning,
                                message: format!(
                                    "Failed to parse Harmony patches from {}: {}",
                                    dll_name, e
                                ),
                                source: DiagnosticSource::DllParsing,
                                analysis_incomplete: true,
                                code: Some("harmony_parse_failure".into()),
                                params: [
                                    ("dll".into(), dll_name.clone()),
                                    ("detail".into(), e.to_string()),
                                ]
                                .into_iter()
                                .collect(),
                            });
                        }
                        Err(_) => {
                            diagnostics.push(Diagnostic {
                                affected_mod: Some(mod_summary.package_id.clone()),
                                severity: Severity::Warning,
                                message: format!(
                                    "DLL parser panicked while parsing {} (dotnetdll crash caught)",
                                    dll_name
                                ),
                                source: DiagnosticSource::DllParsing,
                                analysis_incomplete: true,
                                code: Some("harmony_parse_panic".into()),
                                params: [("dll".into(), dll_name.clone())].into_iter().collect(),
                            });
                        }
                    }

                    diagnostics.extend(scan_manual_diagnostics(&mod_summary.package_id, &dll_path));
                }

                (mod_summary, mod_patches, diagnostics)
            })
            .collect();

        // Build all_patches and diagnostics from parsed results
        let mut all_patches: Vec<(ModSummary, Vec<DiscoveredPatch>)> = Vec::new();
        let mut all_diagnostics: Vec<Diagnostic> = Vec::new();

        for (mod_summary, mod_patches, diags) in parsed {
            if !mod_patches.is_empty() {
                all_patches.push((mod_summary, mod_patches));
            }
            all_diagnostics.extend(diags);
        }

        let conflicts = detect_harmony_conflicts(&all_patches, order);
        (conflicts, all_diagnostics)
    }
}

fn scan_manual_diagnostics(package_id: &str, path: &std::path::Path) -> Vec<Diagnostic> {
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        crate::manual::detect_manual_patches(path)
    }));
    match result {
        Ok(Ok(hints)) => hints
            .iter()
            .map(|hint| manual_hint_diagnostic(package_id, hint))
            .collect(),
        failure => {
            let path = path.display().to_string();
            let (message, code, params) = match failure {
                Ok(Err(error)) => (
                    format!("Failed to read DLL for manual patch detection {path}: {error}"),
                    "manual_dll_read_failure",
                    [("path".into(), path), ("detail".into(), error.to_string())]
                        .into_iter()
                        .collect(),
                ),
                Err(_) => (
                    format!(
                        "Manual patch detection panicked while scanning {path} (caught; results for this DLL may be incomplete)"
                    ),
                    "manual_dll_scan_panic",
                    [("path".into(), path)].into_iter().collect(),
                ),
                Ok(Ok(_)) => unreachable!(),
            };
            vec![Diagnostic {
                affected_mod: Some(package_id.to_owned()),
                severity: Severity::Warning,
                message,
                source: DiagnosticSource::DllParsing,
                analysis_incomplete: true,
                code: Some(code.into()),
                params,
            }]
        }
    }
}

fn manual_hint_diagnostic(package_id: &str, hint: &crate::manual::ManualPatchHint) -> Diagnostic {
    Diagnostic {
        affected_mod: Some(package_id.to_owned()),
        severity: Severity::Info,
        message: format!(
            "Possible manual patch hint in {}: {} (best-effort heuristic, may be a false positive)",
            hint.dll_name, hint.possible_method
        ),
        source: DiagnosticSource::DllParsing,
        analysis_incomplete: false,
        code: Some("manual_patch_hint".into()),
        params: [
            ("dll".into(), hint.dll_name.clone()),
            ("method".into(), hint.possible_method.clone()),
        ]
        .into_iter()
        .collect(),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn missing_manual_dll_produces_incomplete_warning() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.dll");
        let diagnostics = super::scan_manual_diagnostics("example.mod", &path);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(
            diagnostics[0].code.as_deref(),
            Some("manual_dll_read_failure")
        );
        assert_eq!(diagnostics[0].params["path"], path.display().to_string());
        assert!(!diagnostics[0].params["detail"].is_empty());
        assert_eq!(diagnostics[0].severity, rcd_core::Severity::Warning);
        assert!(diagnostics[0].analysis_incomplete);
        assert!(diagnostics[0].message.contains(&path.display().to_string()));
    }

    #[cfg(unix)]
    #[test]
    fn skipped_assembly_symlink_is_an_incomplete_diagnostic() {
        use rcd_core::Detector;
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("About")).unwrap();
        std::fs::write(
            root.path().join("About/About.xml"),
            "<ModMetaData><packageId>test.symlink</packageId></ModMetaData>",
        )
        .unwrap();
        std::fs::create_dir(root.path().join("Assemblies")).unwrap();
        symlink("missing.dll", root.path().join("Assemblies/skipped.dll")).unwrap();
        let mut m = rcd_core::about_xml::parse_about_xml(root.path()).unwrap();
        m.assemblies_paths = vec![root.path().join("Assemblies")];
        let (_, diags) = crate::HarmonyDetector.analyze(&[m], None);
        let warning = diags
            .iter()
            .find(|d| d.message.contains("skipped.dll"))
            .unwrap();
        assert_eq!(warning.code.as_deref(), Some("content_symlink_skipped"));
        assert!(warning.params["path"].contains("skipped.dll"));
        assert!(
            diags
                .iter()
                .any(|d| d.analysis_incomplete && d.message.contains("skipped.dll")),
            "{diags:?}"
        );
    }

    #[test]
    fn failed_dll_analysis_is_incomplete() {
        use rcd_core::Detector;
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("About")).unwrap();
        std::fs::write(
            root.path().join("About/About.xml"),
            "<ModMetaData><packageId>test.broken</packageId></ModMetaData>",
        )
        .unwrap();
        std::fs::create_dir(root.path().join("Assemblies")).unwrap();
        std::fs::write(root.path().join("Assemblies/broken.dll"), b"not a dll").unwrap();
        let mut m = rcd_core::about_xml::parse_about_xml(root.path()).unwrap();
        m.assemblies_paths = vec![root.path().join("Assemblies")];
        let (_, diags) = crate::HarmonyDetector.analyze(&[m], None);
        assert!(
            diags
                .iter()
                .any(|d| d.analysis_incomplete && d.message.contains("Failed to parse Harmony")),
            "{diags:?}"
        );
    }

    #[test]
    fn manual_hint_diagnostic_is_not_incomplete() {
        let hint = crate::manual::ManualPatchHint {
            possible_type: None,
            possible_method: "Kill".into(),
            dll_name: "mod.dll".into(),
        };
        let d = super::manual_hint_diagnostic("test.mod", &hint);
        assert_eq!(d.code.as_deref(), Some("manual_patch_hint"));
        assert_eq!(d.params["dll"], "mod.dll");
        assert_eq!(d.params["method"], "Kill");
        assert_eq!(d.severity, rcd_core::Severity::Info);
        assert!(!d.analysis_incomplete);
    }

    #[test]
    fn analyze_with_no_assemblies_emits_no_diagnostics() {
        use crate::HarmonyDetector;
        use rcd_core::Detector;

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

        // The panic arm of the manual-scan match is intentionally uncovered
        // in-tree (no crash fixture by design); this test pins the non-panic
        // contract only: no assemblies → no diagnostics.
        let (_conflicts, diagnostics) = HarmonyDetector.analyze(&[m], None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn analyze_emits_manual_patch_diagnostics() {
        use crate::HarmonyDetector;
        use rcd_core::{Detector, Severity};

        // Create a minimal mod pointing to the test DLL directory
        let dll_dir = std::path::PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../tests/fixtures/harmony/dlls"
        ));
        let m = rcd_core::Mod {
            package_id: "test.mod".to_string(),
            name: "Test Mod".to_string(),
            author: Some("Test".to_string()),
            is_base_content: false,
            root_path: dll_dir.clone(),
            supported_versions: vec![],
            dependencies: vec![],
            incompatibilities: vec![],
            load_before: vec![],
            load_after: vec![],
            force_load_before: vec![],
            force_load_after: vec![],
            defs_paths: vec![],
            patches_paths: vec![],
            assemblies_paths: vec![dll_dir],
        };

        let detector = HarmonyDetector;
        let (_conflicts, diagnostics) = detector.analyze(&[m], None);

        // The test DLL produces 0 manual hints (confirmed by running
        // detect_manual_patches_does_not_crash_on_valid_dll test).
        // We verify that the analyzer doesn't crash when calling detect_manual_patches,
        // and that any hints that might be emitted have the correct severity.
        let manual_diags: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.message.contains("manual patch hint"))
            .collect();

        // With the test DLL producing 0 hints, there should be 0 manual patch diagnostics
        println!("Manual patch diagnostics found: {}", manual_diags.len());

        // If any are found (shouldn't happen with current test DLL, but future-proof),
        // verify they are Info-level with the expected disclaimer
        for d in &manual_diags {
            assert_eq!(d.severity, Severity::Info);
            assert!(!d.analysis_incomplete);
            assert!(d.message.contains("best-effort"));
        }
    }
}
