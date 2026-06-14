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
                let package_id = mod_data.package_id.clone();

                // Parse defs
                let mut defs = Vec::new();
                for defs_path in &mod_data.defs_paths {
                    defs.extend(def_parser::parse_defs_dir(defs_path));
                }

                // Parse patches
                let mut patches = Vec::new();
                let mut diagnostics = Vec::new();
                for patches_path in &mod_data.patches_paths {
                    match patch_ops::parse_patches_dir(patches_path) {
                        Ok(parsed_patches) => patches.extend(parsed_patches),
                        Err(e) => {
                            diagnostics.push(Diagnostic {
                                affected_mod: Some(package_id.clone()),
                                severity: Severity::Warning,
                                message: format!(
                                    "Failed to parse patches in {}: {}",
                                    patches_path.display(),
                                    e
                                ),
                                source: DiagnosticSource::XmlParsing,
                            });
                        }
                    }
                }

                (mod_summary, package_id, defs, patches, diagnostics)
            })
            .collect();

        // Build mod_defs and mod_patches lists from parsed results
        let mut all_mod_defs = Vec::new();
        let mut all_mod_patches = Vec::new();
        let mut all_diagnostics = Vec::new();

        for (mod_summary, package_id, defs, patches, diagnostics) in parsed {
            all_mod_defs.push(replacement::ModDefs {
                mod_summary: mod_summary.clone(),
                package_id,
                defs,
            });

            if !patches.is_empty() {
                all_mod_patches.push((mod_summary, patches));
            }

            all_diagnostics.extend(diagnostics);
        }

        // 3. Detect direct replacement conflicts
        let replacement_conflicts = replacement::detect_replacements(&all_mod_defs, order);
        all_conflicts.extend(replacement_conflicts);

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
            map.entry(key)
                .or_default()
                .push(mod_defs.mod_summary.clone());
        }
    }

    // Only keep entries with 2+ mods
    map.retain(|_, mods| mods.len() >= 2);
    map
}

#[cfg(test)]
mod integration_tests {
    use super::*;
    use rcd_core::folder::discover_mods;

    fn fixtures_path() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/mods")
    }

    #[test]
    fn xml_detector_finds_steel_conflict() {
        let mods = discover_mods(&[fixtures_path()], Some("1.5"));
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
}
