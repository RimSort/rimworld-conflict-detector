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

    // Search directories: Data/ (Core/DLC), Mods/ (local), and Steam Workshop
    let mut search_dirs = vec![rimworld_path.join("Data"), rimworld_path.join("Mods")];

    // Auto-detect Steam Workshop path by walking up from rimworld_path
    // to find steamapps/workshop/content/294100/
    if let Some(workshop) = find_steam_workshop(rimworld_path) {
        search_dirs.push(workshop);
    }

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

/// Walk up from the RimWorld install path to find the Steam Workshop content directory.
/// RimWorld's Steam app ID is 294100.
///
/// Typical layout:
///   steamapps/common/RimWorld/          (or RimWorldMac.app/)
///   steamapps/workshop/content/294100/  (workshop mods)
fn find_steam_workshop(rimworld_path: &Path) -> Option<PathBuf> {
    let mut current = rimworld_path.to_path_buf();
    while let Some(parent) = current.parent() {
        if parent.file_name().is_some_and(|n| n == "steamapps") {
            let workshop = parent.join("workshop/content/294100");
            if workshop.is_dir() {
                return Some(workshop);
            }
        }
        current = parent.to_path_buf();
    }
    None
}
