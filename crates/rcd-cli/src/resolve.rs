use rcd_core::about_xml::parse_about_xml;
use rcd_core::diagnostic::{Diagnostic, DiagnosticSource};
use rcd_core::folder::{discover_mods, resolve_mod_folders};
use rcd_core::severity::Severity;
use rcd_core::types::{LoadOrder, Mod};
use std::path::{Path, PathBuf};

pub struct ResolvedMods {
    pub mods: Vec<Mod>,
    pub load_order: Option<LoadOrder>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Resolve mods from a directory scan (order-agnostic).
pub fn resolve_from_dir(dir: &Path, game_version: Option<&str>) -> ResolvedMods {
    let mods = discover_mods(&[dir.to_path_buf()], game_version);
    ResolvedMods {
        mods,
        load_order: None,
        diagnostics: vec![],
    }
}

/// Resolve mods from explicit paths.
pub fn resolve_from_mod_paths(
    paths: &[PathBuf],
    game_version: Option<&str>,
    ordered: bool,
) -> ResolvedMods {
    let mut mods = Vec::new();
    let mut diagnostics = Vec::new();

    for path in paths {
        match parse_about_xml(path) {
            Ok(mut m) => {
                resolve_mod_folders(&mut m, game_version, None);
                mods.push(m);
            }
            Err(e) => diagnostics.push(Diagnostic {
                affected_mod: None,
                severity: Severity::Error,
                message: format!("Failed to parse mod at {:?}: {}", path, e),
                source: DiagnosticSource::MetadataResolution,
            }),
        }
    }

    let load_order = if ordered {
        Some(LoadOrder {
            package_ids: mods.iter().map(|m| m.package_id.clone()).collect(),
        })
    } else {
        None
    };

    ResolvedMods {
        mods,
        load_order,
        diagnostics,
    }
}

/// Resolve mods from ModsConfig.xml (order-aware).
pub fn resolve_from_config(
    config_path: &Path,
    rimworld_path: &Path,
    game_version: Option<&str>,
) -> ResolvedMods {
    let config = match rcd_core::mods_config::parse_mods_config(config_path) {
        Ok(c) => c,
        Err(e) => {
            return ResolvedMods {
                mods: vec![],
                load_order: None,
                diagnostics: vec![Diagnostic {
                    affected_mod: None,
                    severity: Severity::Error,
                    message: format!("Failed to parse ModsConfig.xml: {}", e),
                    source: DiagnosticSource::ModsConfig,
                }],
            };
        }
    };

    let version = game_version.map(String::from).or(config.version.clone());

    // Search directories: Data/ (Core/DLC), Mods/ (local), and potentially Steam Workshop
    let search_dirs = vec![rimworld_path.join("Data"), rimworld_path.join("Mods")];
    let available = discover_mods(&search_dirs, version.as_deref());

    // Build packageId -> Mod map
    let mut mod_map: std::collections::HashMap<String, Mod> = std::collections::HashMap::new();
    for m in available {
        mod_map.insert(m.package_id.clone(), m);
    }

    // Resolve active mods in load order
    let mut mods = Vec::new();
    let mut diagnostics = Vec::new();
    for package_id in &config.active_mods {
        match mod_map.remove(package_id) {
            Some(mut m) => {
                if m.package_id.starts_with("ludeon.") {
                    m.is_base_content = true;
                }
                mods.push(m);
            }
            None => diagnostics.push(Diagnostic {
                affected_mod: Some(package_id.clone()),
                severity: Severity::Warning,
                message: format!("Mod '{}' not found in search directories", package_id),
                source: DiagnosticSource::MetadataResolution,
            }),
        }
    }

    ResolvedMods {
        mods,
        load_order: Some(config.load_order()),
        diagnostics,
    }
}
