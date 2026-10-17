use rcd_core::about_xml::parse_about_xml;
use rcd_core::content_files::mark_base_content;
use rcd_core::diagnostic::{Diagnostic, DiagnosticSource, about_parse_failure};
use rcd_core::folder::{discover_mods, discover_mods_with_active, resolve_mod_folders_with_active};
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
    if !dir.is_dir() {
        // discover_mods silently skips absent search dirs by design (correct
        // for --config's incidental Data/Mods/Workshop probing), but an
        // explicit --dir is user-named input: report it as Error so a typo'd
        // path cannot exit 0 as a false clean.
        return ResolvedMods {
            mods: vec![],
            load_order: None,
            diagnostics: vec![Diagnostic {
                affected_mod: None,
                severity: Severity::Error,
                message: if dir.exists() {
                    format!("Mod directory is not a directory: {}", dir.display())
                } else {
                    format!("Mod directory does not exist: {}", dir.display())
                },
                source: DiagnosticSource::MetadataResolution,
            }],
        };
    }
    let (mut mods, diagnostics) = discover_mods(&[dir.to_path_buf()], game_version);
    mark_base_content(&mut mods);
    ResolvedMods {
        mods,
        load_order: None,
        diagnostics,
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
            Ok(m) => {
                mods.push(m);
            }
            Err(e) => diagnostics.push(about_parse_failure(
                path,
                &e,
                Severity::Error,
                DiagnosticSource::MetadataResolution,
            )),
        }
    }

    let active = mods
        .iter()
        .map(rcd_core::ModSummary::from)
        .collect::<Vec<_>>();
    let mut resolution_diagnostics = Vec::new();
    for m in &mut mods {
        resolution_diagnostics.extend(resolve_mod_folders_with_active(m, game_version, &active));
    }
    diagnostics.extend(resolution_diagnostics);
    mark_base_content(&mut mods);

    let load_order = if ordered {
        Some(LoadOrder::new(
            mods.iter().map(|m| m.package_id.clone()).collect(),
        ))
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

    let (available, mut diagnostics) =
        discover_mods_with_active(&search_dirs, version.as_deref(), &config.active_mods);

    // Build packageId -> Mod map
    let mut mod_map: std::collections::HashMap<String, Mod> = std::collections::HashMap::new();
    for m in available {
        mod_map.insert(m.package_id.clone(), m);
    }

    // Resolve active mods in load order
    let mut mods = Vec::new();
    for package_id in &config.active_mods {
        match mod_map.remove(package_id) {
            Some(m) => {
                mods.push(m);
            }
            None => diagnostics.push(Diagnostic {
                affected_mod: Some(package_id.clone()),
                severity: Severity::Warning,
                message: format!(
                    "Mod '{}' not found in search directories (a mod folder may have failed to parse — see \"Failed to parse mod at\" diagnostics in this report)",
                    package_id
                ),
                source: DiagnosticSource::MetadataResolution,
            }),
        }
    }

    mark_base_content(&mut mods);

    // Discovery resolved every candidate mod, but only the retained set is
    // reported in `mods_scanned`. Drop exactly that surplus: FolderResolution
    // diagnostics scoped to a mod that was not retained. Unscoped discovery
    // warnings, retained mods' folder diagnostics, and the MetadataResolution
    // warnings for unresolved active ids all stay.
    let retained: std::collections::HashSet<&str> =
        mods.iter().map(|m| m.package_id.as_str()).collect();
    diagnostics.retain(|d| {
        !(d.source == DiagnosticSource::FolderResolution
            && d.affected_mod
                .as_deref()
                .is_some_and(|id| !retained.contains(id)))
    });

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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_mod(root: &Path, folder: &str, package: &str, name: &str, folders: &str) -> PathBuf {
        let path = root.join(folder);
        std::fs::create_dir_all(path.join("About")).unwrap();
        std::fs::write(
            path.join("About/About.xml"),
            format!(
                "<ModMetaData><packageId>{package}</packageId><name>{name}</name></ModMetaData>"
            ),
        )
        .unwrap();
        std::fs::write(path.join("LoadFolders.xml"), folders).unwrap();
        std::fs::create_dir_all(path.join("Selected/Defs")).unwrap();
        path
    }

    #[test]
    fn dir_and_explicit_mods_use_all_inputs_as_active() {
        let root = tempfile::tempdir().unwrap();
        create_mod(
            root.path(),
            "one",
            "test.one",
            "One",
            "<loadFolders><v1.5><li IfModActive=\"test.two\">Selected</li></v1.5></loadFolders>",
        );
        create_mod(
            root.path(),
            "two",
            "test.two",
            "Two",
            "<loadFolders><v1.5><li>Selected</li></v1.5></loadFolders>",
        );
        let dir_result = resolve_from_dir(root.path(), Some("1.5"));
        assert!(
            dir_result
                .mods
                .iter()
                .find(|m| m.package_id == "test.one")
                .unwrap()
                .defs_paths
                .iter()
                .any(|p| p.ends_with("Selected/Defs"))
        );
        let paths = [root.path().join("one"), root.path().join("two")];
        let mods_result = resolve_from_mod_paths(&paths, Some("1.5"), false);
        assert!(
            mods_result
                .mods
                .iter()
                .find(|m| m.package_id == "test.one")
                .unwrap()
                .defs_paths
                .iter()
                .any(|p| p.ends_with("Selected/Defs"))
        );
    }

    #[test]
    fn config_resolves_conditions_from_only_active_mods() {
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("Data");
        let mods_dir = root.path().join("Mods");
        create_mod(
            &data,
            "One",
            "test.one",
            "One",
            "<loadFolders><v1.5><li IfModActive=\"test.two\">Selected</li></v1.5></loadFolders>",
        );
        create_mod(
            &mods_dir,
            "Two",
            "test.two",
            "Two",
            "<loadFolders><v1.5><li>Selected</li></v1.5></loadFolders>",
        );
        let config = root.path().join("ModsConfig.xml");
        std::fs::write(&config, "<ModsConfigData><version>1.5</version><activeMods><li>test.one</li></activeMods></ModsConfigData>").unwrap();
        let resolved = resolve_from_config(&config, root.path(), None);
        assert_eq!(resolved.mods.len(), 1);
        let one = &resolved.mods[0];
        assert_eq!(one.package_id, "test.one");
        assert!(
            one.defs_paths.is_empty(),
            "inactive discovered mod must not satisfy IfModActive"
        );
    }

    #[test]
    fn resolve_from_dir_surfaces_broken_mod_alongside_good_ones() {
        let dir = tempfile::tempdir().unwrap();
        let good = dir.path().join("GoodMod/About");
        std::fs::create_dir_all(&good).unwrap();
        std::fs::write(
            good.join("About.xml"),
            "<ModMetaData><packageId>test.good</packageId><name>Good</name><author>a</author><supportedVersions><li>1.5</li></supportedVersions></ModMetaData>",
        )
        .unwrap();
        let broken = dir.path().join("BrokenMod/About");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("About.xml"), "<ModMetaData><unclosed>").unwrap();

        let resolved = resolve_from_dir(dir.path(), Some("1.5"));
        assert_eq!(resolved.mods.len(), 1);
        assert_eq!(resolved.mods[0].package_id, "test.good");
        assert_eq!(resolved.diagnostics.len(), 1);
        assert_eq!(
            resolved.diagnostics[0].source,
            rcd_core::DiagnosticSource::FolderResolution
        );
        assert!(
            resolved.diagnostics[0]
                .message
                .contains("Failed to parse mod at")
        );
    }

    #[test]
    fn resolve_from_dir_reports_nonexistent_directory_as_error() {
        // FR-6b: a typo'd explicit --dir must not exit clean. discover_mods
        // silently skips absent dirs (correct for --config's incidental
        // probing), but --dir is user-named input: Error/MetadataResolution,
        // which the exit ladder maps to 2.
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("does-not-exist");
        let resolved = resolve_from_dir(&missing, None);
        assert!(resolved.mods.is_empty());
        assert_eq!(resolved.diagnostics.len(), 1);
        assert_eq!(resolved.diagnostics[0].severity, Severity::Error);
        assert_eq!(
            resolved.diagnostics[0].source,
            rcd_core::DiagnosticSource::MetadataResolution
        );
        assert!(
            resolved.diagnostics[0]
                .message
                .contains("Mod directory does not exist"),
            "absent path must say 'does not exist': {}",
            resolved.diagnostics[0].message
        );
        assert!(
            resolved.diagnostics[0]
                .message
                .contains(&missing.display().to_string())
        );
    }

    #[test]
    fn resolve_from_dir_reports_file_path_as_not_a_directory() {
        // M-3: an EXISTING regular file passed as --dir must not be reported
        // as "does not exist".
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("not_a_dir");
        std::fs::write(&file, "x").unwrap();
        let resolved = resolve_from_dir(&file, None);
        assert!(resolved.mods.is_empty());
        assert_eq!(resolved.diagnostics.len(), 1);
        assert_eq!(resolved.diagnostics[0].severity, Severity::Error);
        assert!(
            resolved.diagnostics[0]
                .message
                .contains("Mod directory is not a directory"),
            "existing file must say 'is not a directory': {}",
            resolved.diagnostics[0].message
        );
    }

    #[test]
    fn resolve_from_config_broken_mod_reports_parse_warning_and_not_found_pair() {
        // Pins the documented --config behavior: a broken mod folder yields
        // BOTH the parse warning (from discovery) and the not-found warning
        // (the mod never enters mod_map because its packageId is unknown).
        let root = tempfile::tempdir().unwrap();
        let mods_dir = root.path().join("Mods");
        let broken = mods_dir.join("BrokenMod/About");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("About.xml"), "<ModMetaData><unclosed>").unwrap();
        let config = root.path().join("ModsConfig.xml");
        std::fs::write(
            &config,
            "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ModsConfigData><version>1.5</version><activeMods><li>BrokenMod</li></activeMods></ModsConfigData>",
        )
        .unwrap();

        let resolved = resolve_from_config(&config, root.path(), Some("1.5"));
        assert!(resolved.mods.is_empty());
        assert_eq!(
            resolved.diagnostics.len(),
            2,
            "parse warning + not-found warning: {:?}",
            resolved
                .diagnostics
                .iter()
                .map(|d| &d.message)
                .collect::<Vec<_>>()
        );
        // Pin the PAIR by source, not by self-referential substrings: [0] is
        // the discovery parse warning, [1] the not-found warning carrying the
        // lowercased activeMods id (mods_config normalizes case).
        assert_eq!(
            resolved.diagnostics[0].source,
            rcd_core::DiagnosticSource::FolderResolution
        );
        assert_eq!(
            resolved.diagnostics[1].source,
            rcd_core::DiagnosticSource::MetadataResolution
        );
        assert_eq!(
            resolved.diagnostics[1].affected_mod.as_deref(),
            Some("brokenmod")
        );
    }

    #[test]
    fn config_drops_folder_diagnostics_for_mods_that_are_not_scanned() {
        // Qwen review I-2: discovery resolves every candidate, but only the
        // configured active set reaches `mods_scanned`. Publishing folder
        // diagnostics for discarded candidates breaks the report contract and
        // can flip the exit code to 2 for an intentionally empty load order.
        let root = tempfile::tempdir().unwrap();
        let mods_dir = root.path().join("Mods");
        create_mod(
            &mods_dir,
            "Inactive",
            "test.inactive",
            "Inactive",
            "<loadFolders><v1.5><li>broken</loadFolders>",
        );
        let config = root.path().join("ModsConfig.xml");
        std::fs::write(
            &config,
            "<ModsConfigData><version>1.5</version><activeMods></activeMods></ModsConfigData>",
        )
        .unwrap();

        let resolved = resolve_from_config(&config, root.path(), None);
        assert!(resolved.mods.is_empty());
        assert!(
            resolved.diagnostics.is_empty(),
            "unscanned candidates must not contribute diagnostics: {:?}",
            resolved
                .diagnostics
                .iter()
                .map(|d| (&d.source, d.affected_mod.as_deref(), d.message.as_str()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn config_keeps_active_folder_diagnostics_and_drops_inactive_ones() {
        let root = tempfile::tempdir().unwrap();
        let mods_dir = root.path().join("Mods");
        let malformed = "<loadFolders><v1.5><li>broken</loadFolders>";
        create_mod(&mods_dir, "Active", "test.active", "Active", malformed);
        create_mod(
            &mods_dir,
            "Inactive",
            "test.inactive",
            "Inactive",
            malformed,
        );
        let config = root.path().join("ModsConfig.xml");
        std::fs::write(
            &config,
            "<ModsConfigData><version>1.5</version><activeMods><li>test.active</li></activeMods></ModsConfigData>",
        )
        .unwrap();

        let resolved = resolve_from_config(&config, root.path(), None);
        assert_eq!(resolved.mods.len(), 1);
        assert_eq!(
            resolved
                .diagnostics
                .iter()
                .map(|d| (&d.source, d.affected_mod.as_deref()))
                .collect::<Vec<_>>(),
            vec![(
                &rcd_core::DiagnosticSource::FolderResolution,
                Some("test.active")
            )],
            "only the retained mod's folder diagnostic may survive: {:?}",
            resolved
                .diagnostics
                .iter()
                .map(|d| d.message.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn config_keeps_metadata_warning_for_an_active_mod_that_was_not_found() {
        // Guards against the reviewer's blanket `retain` by scanned ids: an
        // unresolved ACTIVE mod carries affected_mod but is not in `mods`.
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("ModsConfig.xml");
        std::fs::write(
            &config,
            "<ModsConfigData><version>1.5</version><activeMods><li>test.absent</li></activeMods></ModsConfigData>",
        )
        .unwrap();

        let resolved = resolve_from_config(&config, root.path(), None);
        assert!(resolved.mods.is_empty());
        assert_eq!(resolved.diagnostics.len(), 1);
        assert_eq!(
            resolved.diagnostics[0].source,
            rcd_core::DiagnosticSource::MetadataResolution
        );
        assert_eq!(
            resolved.diagnostics[0].affected_mod.as_deref(),
            Some("test.absent")
        );
    }

    #[test]
    fn mark_base_content_identifies_ludeon_mods() {
        let mut mods = vec![
            Mod {
                package_id: "ludeon.rimworld".to_string(),
                name: "Core".to_string(),
                author: None,
                is_base_content: false,
                root_path: PathBuf::new(),
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
            },
            Mod {
                package_id: "someauthor.coolmod".to_string(),
                name: "Cool Mod".to_string(),
                author: None,
                is_base_content: false,
                root_path: PathBuf::new(),
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
            },
            Mod {
                package_id: "ludeon.rimworld.royalty".to_string(),
                name: "Royalty".to_string(),
                author: None,
                is_base_content: false,
                root_path: PathBuf::new(),
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
            },
        ];

        mark_base_content(&mut mods);

        assert!(
            mods[0].is_base_content,
            "ludeon.rimworld should be base content"
        );
        assert!(
            !mods[1].is_base_content,
            "someauthor.coolmod should not be base content"
        );
        assert!(
            mods[2].is_base_content,
            "ludeon.rimworld.royalty should be base content"
        );
    }
}
