use crate::about_xml::parse_about_xml;
use crate::load_folders::{LoadFolders, ModIdentity, normalize_game_version};
use crate::types::{Mod, ModSummary};
use std::path::{Path, PathBuf};

/// Compatibility resolver for callers that cannot consume folder diagnostics.
/// New resolution paths should use `resolve_mod_folders_with_active`.
pub fn resolve_mod_folders(
    m: &mut Mod,
    game_version: Option<&str>,
    _active_mods: Option<&[String]>,
) {
    resolve_implicit_folders(m, game_version);
}

/// Resolve folder paths against active mod identities and return diagnostics.
pub fn resolve_mod_folders_with_active(
    m: &mut Mod,
    game_version: Option<&str>,
    active_mods: &[ModSummary],
) -> Vec<crate::Diagnostic> {
    let active = active_mods
        .iter()
        .map(|summary| ModIdentity {
            package_id: summary.package_id.clone(),
            display_name: summary.name.clone(),
        })
        .collect::<Vec<_>>();
    let mut diagnostics = Vec::new();
    let is_version_1_0 = game_version.and_then(normalize_game_version) == Some((1, 0));
    if !is_version_1_0 && let Some(path) = find_load_folders(&m.root_path) {
        let parsed = LoadFolders::parse_file(&path);
        let selection = match &parsed {
            Ok(folders) => folders.select(game_version, &active),
            Err(error) => crate::load_folders::FolderSelection {
                entries: None,
                diagnostic: Some(crate::Diagnostic {
                    affected_mod: None,
                    severity: crate::Severity::Warning,
                    message: format!(
                        "Failed to parse LoadFolders.xml; using implicit folder resolution: {error}"
                    ),
                    source: crate::DiagnosticSource::FolderResolution,
                }),
            },
        };
        if let Some(mut diagnostic) = selection.diagnostic {
            diagnostic.affected_mod = Some(m.package_id.clone());
            diagnostics.push(diagnostic);
        }
        if let Ok(folders) = parsed
            && !folders.unknown_conditions.is_empty()
        {
            diagnostics.push(crate::Diagnostic {
                affected_mod: Some(m.package_id.clone()),
                severity: crate::Severity::Info,
                message: format!(
                    "Unknown LoadFolders.xml condition attributes: {}",
                    folders.unknown_conditions.join(", ")
                ),
                source: crate::DiagnosticSource::FolderResolution,
            });
        }
        if let Some(entries) = selection.entries {
            clear_paths(m);
            for entry in entries.iter().rev() {
                if let Some(relative) = entry.relative_path() {
                    add_content_dirs(m, &relative);
                }
            }
            return diagnostics;
        }
    }
    resolve_implicit_folders(m, game_version);
    diagnostics
}

fn clear_paths(m: &mut Mod) {
    m.defs_paths.clear();
    m.patches_paths.clear();
    m.assemblies_paths.clear();
}

fn find_load_folders(root: &Path) -> Option<PathBuf> {
    let mut entries = std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.file_name());
    entries.into_iter().find_map(|entry| {
        entry
            .file_type()
            .ok()?
            .is_file()
            .then(|| entry.path())
            .filter(|path| {
                path.file_name().is_some_and(|name| {
                    name.to_string_lossy()
                        .eq_ignore_ascii_case("LoadFolders.xml")
                })
            })
    })
}

fn add_content_dirs(m: &mut Mod, relative: &Path) {
    for (folder, paths) in [
        ("Defs", &mut m.defs_paths),
        ("Patches", &mut m.patches_paths),
        ("Assemblies", &mut m.assemblies_paths),
    ] {
        let full = m.root_path.join(relative).join(folder);
        if full.is_dir() {
            paths.push(full);
        }
    }
}

fn resolve_implicit_folders(m: &mut Mod, game_version: Option<&str>) {
    clear_paths(m);
    let version = game_version
        .and_then(normalize_game_version)
        .map(|(major, minor)| format!("{major}.{minor}"));
    for (folder, paths) in [
        ("Defs", &mut m.defs_paths),
        ("Patches", &mut m.patches_paths),
        ("Assemblies", &mut m.assemblies_paths),
    ] {
        if let Some(version) = &version {
            let path = m.root_path.join(version).join(folder);
            if path.is_dir() {
                paths.push(path);
            }
        }
        let common = m.root_path.join("Common").join(folder);
        if common.is_dir() {
            paths.push(common);
        }
        let root = m.root_path.join(folder);
        if root.is_dir() {
            paths.push(root);
        }
    }
}

/// Discover all mods and resolve conditions using the supplied active package IDs.
pub fn discover_mods_with_active(
    search_dirs: &[PathBuf],
    game_version: Option<&str>,
    active_ids: &[String],
) -> (Vec<Mod>, Vec<crate::Diagnostic>) {
    let (mut mods, mut diagnostics) = discover_mod_metadata(search_dirs);
    let mut active = mods
        .iter()
        .filter(|m| {
            active_ids
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&m.package_id))
        })
        .map(ModSummary::from)
        .collect::<Vec<_>>();
    for id in active_ids {
        if !active
            .iter()
            .any(|summary| summary.package_id.eq_ignore_ascii_case(id))
        {
            active.push(ModSummary {
                package_id: id.clone(),
                name: String::new(),
                author: None,
                is_base_content: false,
            });
        }
    }
    for m in &mut mods {
        diagnostics.extend(resolve_mod_folders_with_active(m, game_version, &active));
    }
    (mods, diagnostics)
}

/// Discover mods with every discovered mod active.
pub fn discover_mods(
    search_dirs: &[PathBuf],
    game_version: Option<&str>,
) -> (Vec<Mod>, Vec<crate::Diagnostic>) {
    let (mut mods, mut diagnostics) = discover_mod_metadata(search_dirs);
    let active = mods.iter().map(ModSummary::from).collect::<Vec<_>>();
    for m in &mut mods {
        diagnostics.extend(resolve_mod_folders_with_active(m, game_version, &active));
    }
    (mods, diagnostics)
}

fn discover_mod_metadata(search_dirs: &[PathBuf]) -> (Vec<Mod>, Vec<crate::Diagnostic>) {
    let mut mods = Vec::new();
    let mut diagnostics = Vec::new();
    for search_dir in search_dirs {
        if !search_dir.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(search_dir) {
            Ok(entries) => entries,
            Err(e) => {
                diagnostics.push(discovery_warning(format!(
                    "Failed to read directory {}: {}",
                    search_dir.display(),
                    e
                )));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    diagnostics.push(discovery_warning(format!(
                        "Failed to read directory entry in {}: {}",
                        search_dir.display(),
                        e
                    )));
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_dir() || !path.join("About/About.xml").exists() {
                continue;
            }
            match parse_about_xml(&path) {
                Ok(mod_data) => mods.push(mod_data),
                Err(e) => diagnostics.push(crate::about_parse_failure(
                    &path,
                    &e,
                    crate::Severity::Warning,
                    crate::DiagnosticSource::FolderResolution,
                )),
            }
        }
    }
    (mods, diagnostics)
}

fn discovery_warning(message: String) -> crate::Diagnostic {
    crate::Diagnostic {
        affected_mod: None,
        severity: crate::Severity::Warning,
        message,
        source: crate::DiagnosticSource::FolderResolution,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixtures_path;

    #[test]
    fn resolve_version_common_and_root_together_in_precedence_order() {
        let dir = tempfile::tempdir().unwrap();
        let about = dir.path().join("About");
        std::fs::create_dir_all(about).unwrap();
        std::fs::write(
            dir.path().join("About/About.xml"),
            "<ModMetaData><packageId>x</packageId><name>x</name></ModMetaData>",
        )
        .unwrap();
        for path in ["1.5/Defs", "Common/Defs", "Defs"] {
            std::fs::create_dir_all(dir.path().join(path)).unwrap();
        }
        let mut m = parse_about_xml(dir.path()).unwrap();
        resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        assert_eq!(
            m.defs_paths,
            [
                dir.path().join("1.5/Defs"),
                dir.path().join("Common/Defs"),
                dir.path().join("Defs")
            ]
        );
    }

    fn write_mod(
        root: &Path,
        folder: &str,
        id: &str,
        name: &str,
        load_folders: Option<&str>,
    ) -> PathBuf {
        let path = root.join(folder);
        std::fs::create_dir_all(path.join("About")).unwrap();
        std::fs::write(
            path.join("About/About.xml"),
            format!("<ModMetaData><packageId>{id}</packageId><name>{name}</name></ModMetaData>"),
        )
        .unwrap();
        if let Some(xml) = load_folders {
            std::fs::write(path.join("lOaDfOlDeRs.XmL"), xml).unwrap();
        }
        path
    }

    #[test]
    fn explicit_entries_reverse_precedence_and_ignore_missing_directories() {
        let root = tempfile::tempdir().unwrap();
        let xml =
            "<loadFolders><v1.5><li>First</li><li>Missing</li><li>Last</li></v1.5></loadFolders>";
        let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", Some(xml));
        for path in ["First/Defs", "Last/Defs"] {
            std::fs::create_dir_all(mod_path.join(path)).unwrap();
        }
        let mut m = parse_about_xml(&mod_path).unwrap();
        let diagnostics = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        assert!(diagnostics.is_empty());
        assert_eq!(
            m.defs_paths,
            [mod_path.join("Last/Defs"), mod_path.join("First/Defs")]
        );
    }

    #[test]
    fn malformed_and_unmatched_loadfolders_fall_back_but_empty_selection_stays_empty() {
        for (xml, expected_empty) in [
            ("<loadFolders><v1.5><li>broken</loadFolders>", false),
            (
                "<loadFolders><v1.5><li IfModActive=\"absent\">Only</li></v1.5></loadFolders>",
                true,
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", Some(xml));
            std::fs::create_dir_all(mod_path.join("Defs")).unwrap();
            let mut m = parse_about_xml(&mod_path).unwrap();
            let diagnostics = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].affected_mod.as_deref(), Some("example.mod"));
            assert_eq!(m.defs_paths.is_empty(), expected_empty);
        }
    }

    #[test]
    fn game_version_1_0_uses_implicit_folders_even_with_loadfolders() {
        for version in ["1.0.2654", "1.0.2654 rev44"] {
            let root = tempfile::tempdir().unwrap();
            let mod_path = write_mod(
                root.path(),
                "mod",
                "example.mod",
                "Example",
                Some("<loadFolders><v1.0><li>Custom</li></v1.0></loadFolders>"),
            );
            std::fs::create_dir_all(mod_path.join("Custom/Defs")).unwrap();
            std::fs::create_dir_all(mod_path.join("Defs")).unwrap();
            let mut m = parse_about_xml(&mod_path).unwrap();
            let diagnostics = resolve_mod_folders_with_active(&mut m, Some(version), &[]);
            assert!(diagnostics.is_empty());
            assert_eq!(m.defs_paths, [mod_path.join("Defs")], "version {version}");
        }
    }

    #[test]
    fn implicit_version_paths_use_loadfolders_normalization() {
        for version in ["1.7.4976", "1.7.4976 rev44"] {
            for load_folders in [None, Some("<loadFolders><v1.7><li>Broken")] {
                let root = tempfile::tempdir().unwrap();
                let mod_path =
                    write_mod(root.path(), "mod", "example.mod", "Example", load_folders);
                std::fs::create_dir_all(mod_path.join("1.7/Defs")).unwrap();
                let mut m = parse_about_xml(&mod_path).unwrap();
                let diagnostics = resolve_mod_folders_with_active(&mut m, Some(version), &[]);
                if load_folders.is_some() {
                    assert_eq!(diagnostics.len(), 1);
                } else {
                    assert!(diagnostics.is_empty());
                }
                assert!(
                    m.defs_paths
                        .iter()
                        .any(|path| path == &mod_path.join("1.7/Defs")),
                    "version {version}"
                );
            }
        }
    }

    #[test]
    fn no_applicable_loadfolders_entry_falls_back_to_implicit_paths() {
        let root = tempfile::tempdir().unwrap();
        let mod_path = write_mod(
            root.path(),
            "mod",
            "example.mod",
            "Example",
            Some("<loadFolders><v1.6><li>New</li></v1.6></loadFolders>"),
        );
        std::fs::create_dir_all(mod_path.join("Defs")).unwrap();
        let mut m = parse_about_xml(&mod_path).unwrap();
        let diagnostics = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        assert_eq!(diagnostics.len(), 1);
        assert!(
            m.defs_paths
                .iter()
                .any(|path| path == &mod_path.join("Defs"))
        );
    }

    #[test]
    fn unknown_conditions_are_reported_once_per_mod() {
        let root = tempfile::tempdir().unwrap();
        let mod_path = write_mod(
            root.path(),
            "mod",
            "example.mod",
            "Example",
            Some("<loadFolders><v1.5><li IfNewCondition=\"x\">Defs</li></v1.5></loadFolders>"),
        );
        std::fs::create_dir_all(mod_path.join("Defs")).unwrap();
        let mut m = parse_about_xml(&mod_path).unwrap();
        let diagnostics = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, crate::Severity::Info);
        assert_eq!(diagnostics[0].affected_mod.as_deref(), Some("example.mod"));
    }

    #[test]
    fn discover_mods_finds_fixtures() {
        let (mods, _) = discover_mods(&[fixtures_path()], Some("1.5"));
        assert!(mods.len() >= 2);
        assert!(mods.iter().any(|m| m.package_id == "test.mod.a"));
    }
}
