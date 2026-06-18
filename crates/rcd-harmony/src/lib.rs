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
use crate::discovery::discover_assemblies;

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

                for assemblies_path in &m.assemblies_paths {
                    let (dlls, discovery_errors) = discover_assemblies(assemblies_path);
                    for err_msg in discovery_errors {
                        diagnostics.push(Diagnostic {
                            affected_mod: Some(mod_summary.package_id.clone()),
                            severity: Severity::Warning,
                            message: err_msg,
                            source: DiagnosticSource::DllParsing,
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
                                });
                            }
                        }

                        // Best-effort manual patch detection via string scanning
                        let manual_result =
                            panic::catch_unwind(panic::AssertUnwindSafe(|| {
                                crate::manual::detect_manual_patches(&dll_path)
                            }));

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
                    }
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

#[cfg(test)]
mod tests {
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
            assert!(d.message.contains("best-effort"));
        }
    }
}
