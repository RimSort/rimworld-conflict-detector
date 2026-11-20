use crate::about_xml::parse_about_xml;
use crate::load_folders::{LoadFolders, ModIdentity, normalize_game_version};
use crate::types::{Mod, ModSummary};
use std::path::{Path, PathBuf};

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
    if !is_version_1_0 {
        match find_load_folders(&m.root_path) {
            Ok(Some(path)) => {
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
                            analysis_incomplete: true,
                            code: Some("load_folders_parse_failure".into()),
                            params: [("path".into(), path.display().to_string()), ("detail".into(), error.to_string())].into_iter().collect(),
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
                        analysis_incomplete: true,
                        code: Some("load_folders_unknown_conditions".into()),
                        params: [("conditions".into(), folders.unknown_conditions.join(", "))].into_iter().collect(),
                    });
                }
                if let Some(entries) = selection.entries {
                    clear_paths(m);
                    for entry in entries.iter().rev() {
                        if let Some(relative) = entry.relative_path() {
                            add_content_dirs(m, &relative, &mut diagnostics);
                        }
                    }
                    return diagnostics;
                }
            }
            Ok(None) => {}
            Err(error) => diagnostics.push(crate::Diagnostic {
                affected_mod: Some(m.package_id.clone()),
                severity: crate::Severity::Warning,
                message: format!(
                    "Failed to inspect LoadFolders.xml in {}; using implicit folder resolution: {error}",
                    m.root_path.display()
                ),
                source: crate::DiagnosticSource::FolderResolution,
                analysis_incomplete: true,
                code: Some("load_folders_inspection_failure".into()),
                params: [("path".into(), m.root_path.display().to_string()), ("detail".into(), error.to_string())].into_iter().collect(),
            }),
        }
    }
    resolve_implicit_folders(m, game_version, &mut diagnostics);
    diagnostics
}

fn clear_paths(m: &mut Mod) {
    m.defs_paths.clear();
    m.patches_paths.clear();
    m.assemblies_paths.clear();
}

fn find_load_folders(root: &Path) -> std::io::Result<Option<PathBuf>> {
    let mut entries = std::fs::read_dir(root)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    let candidates = entries
        .into_iter()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("LoadFolders.xml")
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [] => Ok(None),
        [path] => {
            let metadata = std::fs::metadata(path).map_err(|error| {
                std::io::Error::new(error.kind(), format!("{}: {error}", path.display()))
            })?;
            if !metadata.is_file() {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: not a regular file", path.display()),
                ));
            }
            Ok(Some(path.clone()))
        }
        _ => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "multiple LoadFolders.xml candidates: {}",
                candidates
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

fn inspect_content_dir(
    path: PathBuf,
    paths: &mut Vec<PathBuf>,
    mod_id: &str,
    diagnostics: &mut Vec<crate::Diagnostic>,
) {
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_symlink() => std::fs::metadata(&path),
        Ok(metadata) => Ok(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match inspect_missing_parent(&path) {
                Ok(()) => return,
                Err(error) => Err(error),
            }
        }
        Err(error) => Err(error),
    };
    match metadata {
        Ok(metadata) if metadata.is_dir() => paths.push(path),
        other => {
            let (detail, code) = match other {
                Ok(_) => ("not a directory".to_owned(), "content_path_not_directory"),
                Err(error) => (error.to_string(), "content_directory_inspection_failure"),
            };
            diagnostics.push(crate::Diagnostic {
                affected_mod: Some(mod_id.to_owned()),
                severity: crate::Severity::Warning,
                message: format!(
                    "Failed to inspect content directory {}: {detail}",
                    path.display()
                ),
                source: crate::DiagnosticSource::FolderResolution,
                analysis_incomplete: true,
                code: Some(code.into()),
                params: [
                    ("path".into(), path.display().to_string()),
                    ("detail".into(), detail),
                ]
                .into_iter()
                .collect(),
            });
        }
    }
}

fn inspect_missing_parent(path: &Path) -> std::io::Result<()> {
    for parent in path.ancestors().skip(1) {
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) => {
                if metadata.file_type().is_symlink() {
                    std::fs::metadata(parent)?;
                }
                return Ok(());
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn add_content_dirs(m: &mut Mod, relative: &Path, diagnostics: &mut Vec<crate::Diagnostic>) {
    for (folder, paths) in [
        ("Defs", &mut m.defs_paths),
        ("Patches", &mut m.patches_paths),
        ("Assemblies", &mut m.assemblies_paths),
    ] {
        let full = m.root_path.join(relative).join(folder);
        inspect_content_dir(full, paths, &m.package_id, diagnostics);
    }
}

fn resolve_implicit_folders(
    m: &mut Mod,
    game_version: Option<&str>,
    diagnostics: &mut Vec<crate::Diagnostic>,
) {
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
            inspect_content_dir(
                m.root_path.join(version).join(folder),
                paths,
                &m.package_id,
                diagnostics,
            );
        }
        inspect_content_dir(
            m.root_path.join("Common").join(folder),
            paths,
            &m.package_id,
            diagnostics,
        );
        inspect_content_dir(m.root_path.join(folder), paths, &m.package_id, diagnostics);
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
                diagnostics.push(discovery_warning(search_dir, &e, false));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(e) => {
                    diagnostics.push(discovery_warning(search_dir, &e, true));
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

fn discovery_warning(
    path: &Path,
    error: &impl std::fmt::Display,
    entry: bool,
) -> crate::Diagnostic {
    let (message, code) = if entry {
        (
            format!(
                "Failed to read directory entry in {}: {}",
                path.display(),
                error
            ),
            "mod_directory_entry_read_failure",
        )
    } else {
        (
            format!("Failed to read directory {}: {}", path.display(), error),
            "mod_directory_read_failure",
        )
    };
    crate::Diagnostic {
        affected_mod: None,
        severity: crate::Severity::Warning,
        message,
        source: crate::DiagnosticSource::FolderResolution,
        analysis_incomplete: true,
        code: Some(code.into()),
        params: [
            ("path".into(), path.display().to_string()),
            ("detail".into(), error.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixtures_path;

    #[test]
    fn discovery_warning_preserves_path_and_detail() {
        let path = Path::new("/mods");
        let diagnostic = discovery_warning(path, &"bad", false);
        assert_eq!(
            diagnostic.code.as_deref(),
            Some("mod_directory_read_failure")
        );
        assert_eq!(diagnostic.params["path"], "/mods");
        assert_eq!(diagnostic.params["detail"], "bad");
        assert_eq!(diagnostic.message, "Failed to read directory /mods: bad");
    }

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
    #[cfg(unix)]
    fn unique_loadfolders_file_link_selects_custom_content() {
        use std::os::unix::fs::symlink;
        let xml = "<loadFolders><v1.7><li>Custom</li></v1.7></loadFolders>";
        for linked in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let path = write_mod(root.path(), "mod", "example.mod", "Example", None);
            std::fs::create_dir_all(path.join("Custom/Defs")).unwrap();
            if linked {
                std::fs::write(path.join("folders.xml"), xml).unwrap();
                symlink("folders.xml", path.join("LoadFolders.xml")).unwrap();
            } else {
                std::fs::write(path.join("LoadFolders.xml"), xml).unwrap();
            }
            let mut m = parse_about_xml(&path).unwrap();
            let diagnostics = resolve_mod_folders_with_active(&mut m, Some("1.7"), &[]);
            assert!(diagnostics.is_empty());
            assert_eq!(m.defs_paths, [path.join("Custom/Defs")]);
        }
    }

    #[test]
    #[cfg(unix)]
    fn unusable_loadfolders_selector_is_incomplete() {
        use std::os::unix::fs::symlink;
        for kind in ["dangling", "loop", "directory"] {
            let root = tempfile::tempdir().unwrap();
            let path = write_mod(root.path(), "mod", "example.mod", "Example", None);
            std::fs::create_dir_all(path.join("Defs")).unwrap();
            let selector = path.join("LoadFolders.xml");
            match kind {
                "dangling" => symlink("missing.xml", &selector).unwrap(),
                "loop" => symlink("LoadFolders.xml", &selector).unwrap(),
                _ => std::fs::create_dir(&selector).unwrap(),
            }
            let mut m = parse_about_xml(&path).unwrap();
            let diagnostics = resolve_mod_folders_with_active(&mut m, Some("1.7"), &[]);
            let issue = diagnostics
                .iter()
                .find(|d| d.code.as_deref() == Some("load_folders_inspection_failure"))
                .unwrap();
            assert!(issue.analysis_incomplete);
            assert_eq!(issue.severity, crate::Severity::Warning);
            assert!(issue.params["detail"].contains(&selector.display().to_string()));
            assert_eq!(m.defs_paths, [path.join("Defs")]);
        }
    }

    #[test]
    #[cfg(unix)]
    fn ambiguous_loadfolders_selectors_are_incomplete() {
        let root = tempfile::tempdir().unwrap();
        let path = write_mod(root.path(), "mod", "example.mod", "Example", None);
        std::fs::create_dir_all(path.join("Defs")).unwrap();
        let a = path.join("LoadFolders.xml");
        let b = path.join("loadfolders.xml");
        std::fs::write(&a, "<loadFolders/>").unwrap();
        if b.exists() {
            return; // This filesystem cannot represent competing case variants.
        }
        std::fs::write(&b, "<loadFolders/>").unwrap();
        let mut m = parse_about_xml(&path).unwrap();
        let diagnostics = resolve_mod_folders_with_active(&mut m, Some("1.7"), &[]);
        let issue = diagnostics
            .iter()
            .find(|d| d.code.as_deref() == Some("load_folders_inspection_failure"))
            .unwrap();
        assert!(issue.analysis_incomplete);
        assert!(issue.params["detail"].contains(&a.display().to_string()));
        assert!(issue.params["detail"].contains(&b.display().to_string()));
        assert_eq!(m.defs_paths, [path.join("Defs")]);
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
            ("<loadFolders><v1.5><li>Folder</li></loadFolders>", false),
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
            assert_eq!(diagnostics[0].analysis_incomplete, !expected_empty);
            assert_eq!(m.defs_paths.is_empty(), expected_empty);
        }
    }

    #[test]
    fn loadfolders_inspection_error_warns_and_uses_implicit_fallback() {
        let root = tempfile::tempdir().unwrap();
        let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", None);
        std::fs::create_dir_all(mod_path.join("Defs")).unwrap();
        let mut m = parse_about_xml(&mod_path).unwrap();
        std::fs::remove_dir_all(&mod_path).unwrap();
        std::fs::write(&mod_path, "not a directory").unwrap();

        let diagnostics = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        let loadfolders = diagnostics
            .iter()
            .find(|d| d.message.contains("LoadFolders"))
            .unwrap();
        assert_eq!(
            loadfolders.source,
            crate::DiagnosticSource::FolderResolution
        );
        assert_eq!(
            loadfolders.code.as_deref(),
            Some("load_folders_inspection_failure")
        );
        assert_eq!(loadfolders.params["path"], mod_path.display().to_string());
        assert!(!loadfolders.params["detail"].is_empty());
        assert_eq!(loadfolders.severity, crate::Severity::Warning);
        assert_eq!(loadfolders.affected_mod.as_deref(), Some("example.mod"));
        assert!(diagnostics.iter().all(|d| d.analysis_incomplete));
        assert!(m.defs_paths.is_empty());
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
    fn optional_absence_is_normal_but_non_directory_is_incomplete() {
        let root = tempfile::tempdir().unwrap();
        let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", None);
        let mut m = parse_about_xml(&mod_path).unwrap();
        assert!(resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]).is_empty());
        std::fs::write(mod_path.join("Defs"), "not a directory").unwrap();
        let diags = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        let diagnostic = diags.iter().find(|d| d.message.contains("Defs")).unwrap();
        assert_eq!(
            diagnostic.code.as_deref(),
            Some("content_path_not_directory")
        );
        assert_eq!(
            diagnostic.params["path"],
            mod_path.join("Defs").display().to_string()
        );
        assert!(
            diags
                .iter()
                .any(|d| d.analysis_incomplete && d.message.contains("Defs"))
        );
    }

    #[test]
    fn selected_invalid_content_directory_keeps_valid_sibling() {
        let root = tempfile::tempdir().unwrap();
        let xml = "<loadFolders><v1.5><li>Bad</li><li>Good</li></v1.5></loadFolders>";
        let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", Some(xml));
        let bad_defs = mod_path.join("Bad").join("Defs");
        std::fs::create_dir_all(mod_path.join("Bad")).unwrap();
        std::fs::write(&bad_defs, "not a directory").unwrap();
        std::fs::create_dir_all(mod_path.join("Good/Defs")).unwrap();
        let mut m = parse_about_xml(&mod_path).unwrap();
        let diags = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        assert_eq!(m.defs_paths, [mod_path.join("Good/Defs")]);
        assert!(
            diags.iter().any(|d| {
                d.analysis_incomplete
                    && d.code.as_deref() == Some("content_path_not_directory")
                    && d.params
                        .get("path")
                        .is_some_and(|path| Path::new(path) == bad_defs.as_path())
            }),
            "{diags:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn dangling_intermediate_selected_path_is_incomplete_and_keeps_valid_sibling() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let xml = "<loadFolders><v1.5><li>Custom/Deep</li><li>Good</li></v1.5></loadFolders>";
        let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", Some(xml));
        symlink("missing", mod_path.join("Custom")).unwrap();
        std::fs::create_dir_all(mod_path.join("Good/Defs")).unwrap();
        let mut m = parse_about_xml(&mod_path).unwrap();
        let diags = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        assert_eq!(m.defs_paths, [mod_path.join("Good/Defs")]);
        for folder in ["Defs", "Patches", "Assemblies"] {
            let expected = mod_path.join("Custom/Deep").join(folder);
            assert!(
                diags.iter().any(|d| d.analysis_incomplete
                    && d.source == crate::DiagnosticSource::FolderResolution
                    && d.message.contains(&expected.to_string_lossy().to_string())),
                "{diags:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn dangling_intermediate_implicit_paths_are_incomplete_and_keep_valid_sibling() {
        use std::os::unix::fs::symlink;
        for broken in ["Common", "1.5"] {
            let root = tempfile::tempdir().unwrap();
            let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", None);
            symlink("missing", mod_path.join(broken)).unwrap();
            std::fs::create_dir_all(mod_path.join("Defs")).unwrap();
            let mut m = parse_about_xml(&mod_path).unwrap();
            let diags = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
            assert_eq!(m.defs_paths, [mod_path.join("Defs")]);
            for folder in ["Defs", "Patches", "Assemblies"] {
                let expected = mod_path.join(broken).join(folder);
                assert!(
                    diags.iter().any(|d| d.analysis_incomplete
                        && d.source == crate::DiagnosticSource::FolderResolution
                        && d.message.contains(&expected.to_string_lossy().to_string())),
                    "{broken}: {diags:?}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn valid_intermediate_symlink_preserves_optional_absence_and_scans_defs() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let xml = "<loadFolders><v1.5><li>Linked</li></v1.5></loadFolders>";
        let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", Some(xml));
        std::fs::create_dir_all(mod_path.join("Target/Defs")).unwrap();
        symlink("Target", mod_path.join("Linked")).unwrap();
        let mut m = parse_about_xml(&mod_path).unwrap();
        let diags = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        assert!(diags.is_empty(), "{diags:?}");
        assert_eq!(m.defs_paths, [mod_path.join("Linked/Defs")]);
        assert!(m.patches_paths.is_empty());
        assert!(m.assemblies_paths.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn top_level_symlink_loop_is_incomplete() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let mod_path = write_mod(root.path(), "mod", "example.mod", "Example", None);
        symlink("Defs", mod_path.join("Defs")).unwrap();
        let mut m = parse_about_xml(&mod_path).unwrap();
        let diags = resolve_mod_folders_with_active(&mut m, Some("1.5"), &[]);
        assert!(
            diags
                .iter()
                .any(|d| d.analysis_incomplete && d.message.contains("Defs"))
        );
    }

    #[test]
    fn discover_mods_finds_fixtures() {
        let (mods, _) = discover_mods(&[fixtures_path()], Some("1.5"));
        assert!(mods.len() >= 2);
        assert!(mods.iter().any(|m| m.package_id == "test.mod.a"));
    }
}
