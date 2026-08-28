use crate::about_xml::parse_about_xml;
use crate::types::Mod;
use std::path::PathBuf;

/// Resolve folder paths for a mod based on game version.
///
/// Resolution order for Defs, Patches, and Assemblies:
/// 1. Version-specific folder (e.g., `1.5/Defs`, `1.5/Patches`, `1.5/Assemblies`) — highest priority
/// 2. `Common/` folder (`Common/Defs`, `Common/Patches`, `Common/Assemblies`)
/// 3. Root folder (`Defs/`, `Patches/`, `Assemblies/`) — only if no version-specific folder was found
///
/// Sets `m.defs_paths`, `m.patches_paths`, `m.assemblies_paths`.
fn is_valid_game_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    parts.len() == 2
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

pub fn resolve_mod_folders(
    m: &mut Mod,
    game_version: Option<&str>,
    _active_mods: Option<&[String]>,
) {
    m.defs_paths.clear();
    m.patches_paths.clear();
    m.assemblies_paths.clear();

    let game_version = game_version.filter(|v| is_valid_game_version(v));

    // Helper to check if a directory exists and add it to the paths
    let add_if_exists = |paths: &mut Vec<PathBuf>, subpath: &str| {
        let full_path = m.root_path.join(subpath);
        if full_path.is_dir() {
            paths.push(full_path);
        }
    };

    // Resolve for each folder type (Defs, Patches, Assemblies)
    for (folder_name, paths) in [
        ("Defs", &mut m.defs_paths),
        ("Patches", &mut m.patches_paths),
        ("Assemblies", &mut m.assemblies_paths),
    ] {
        let mut found_version_specific = false;

        // 1. Check for version-specific folder
        if let Some(version) = game_version {
            let version_path = format!("{}/{}", version, folder_name);
            let full_path = m.root_path.join(&version_path);
            if full_path.is_dir() {
                paths.push(full_path);
                found_version_specific = true;
            }
        }

        // 2. Check for Common/ folder
        add_if_exists(paths, &format!("Common/{}", folder_name));

        // 3. Check for root folder (only if no version-specific folder was found)
        if !found_version_specific {
            add_if_exists(paths, folder_name);
        }
    }
}

/// Discover mods in the given search directories.
///
/// Iterates through each search directory, finds subdirectories with `About/About.xml`,
/// parses them with `parse_about_xml`, calls `resolve_mod_folders` on each, and returns the list.
pub fn discover_mods(
    search_dirs: &[PathBuf],
    game_version: Option<&str>,
) -> (Vec<Mod>, Vec<crate::Diagnostic>) {
    let mut mods = Vec::new();
    let mut diagnostics = Vec::new();

    for search_dir in search_dirs {
        if !search_dir.is_dir() {
            continue; // absent dir is normal (e.g., no Workshop install) — not an error
        }

        let entries = match std::fs::read_dir(search_dir) {
            Ok(entries) => entries,
            Err(e) => {
                diagnostics.push(crate::Diagnostic {
                    affected_mod: None,
                    severity: crate::Severity::Warning,
                    message: format!("Failed to read directory {}: {}", search_dir.display(), e),
                    source: crate::DiagnosticSource::FolderResolution,
                });
                continue;
            }
        };

        for entry in entries {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    diagnostics.push(crate::Diagnostic {
                        affected_mod: None,
                        severity: crate::Severity::Warning,
                        message: format!(
                            "Failed to read directory entry in {}: {}",
                            search_dir.display(),
                            e
                        ),
                        source: crate::DiagnosticSource::FolderResolution,
                    });
                    continue;
                }
            };
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Check if About/About.xml exists
            let about_xml = path.join("About/About.xml");
            if !about_xml.exists() {
                continue; // directory without About.xml is not a mod — silent by design
            }

            match parse_about_xml(&path) {
                Ok(mut mod_data) => {
                    resolve_mod_folders(&mut mod_data, game_version, None);
                    mods.push(mod_data);
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::fixtures_path;

    #[test]
    fn resolve_version_specific_folder() {
        let mut m = parse_about_xml(&fixtures_path().join("mod_versioned")).unwrap();
        resolve_mod_folders(&mut m, Some("1.5"), None);
        assert!(m.defs_paths.iter().any(|p| p.ends_with("1.5/Defs")));
    }

    #[test]
    fn resolve_common_folder() {
        let mut m = parse_about_xml(&fixtures_path().join("mod_versioned")).unwrap();
        resolve_mod_folders(&mut m, Some("1.5"), None);
        assert!(m.defs_paths.iter().any(|p| p.ends_with("Common/Defs")));
    }

    #[test]
    fn invalid_game_version_treated_as_none() {
        let mut m = parse_about_xml(&fixtures_path().join("mod_versioned")).unwrap();
        resolve_mod_folders(&mut m, Some("../../etc"), None);
        assert!(
            !m.defs_paths
                .iter()
                .any(|p| p.to_string_lossy().contains("etc")),
            "Path traversal in game_version should be rejected"
        );
    }

    #[test]
    fn valid_game_version_formats() {
        assert!(is_valid_game_version("1.5"));
        assert!(is_valid_game_version("1.4"));
        assert!(!is_valid_game_version("../../etc"));
        assert!(!is_valid_game_version("1.5.3"));
        assert!(!is_valid_game_version(""));
        assert!(!is_valid_game_version(".5"));
        assert!(!is_valid_game_version("1."));
    }

    #[test]
    fn discover_mods_finds_fixtures() {
        let (mods, _diagnostics) = discover_mods(&[fixtures_path()], Some("1.5"));
        assert!(mods.len() >= 2);
        assert!(mods.iter().any(|m| m.package_id == "test.mod.a"));
        assert!(mods.iter().any(|m| m.package_id == "test.mod.versioned"));
    }

    #[test]
    fn discover_mods_reports_unparseable_about_xml() {
        let dir = tempfile::tempdir().unwrap();
        let broken = dir.path().join("BrokenMod/About");
        std::fs::create_dir_all(&broken).unwrap();
        std::fs::write(broken.join("About.xml"), "<ModMetaData><unclosed>").unwrap();

        let (mods, diagnostics) = discover_mods(&[dir.path().to_path_buf()], None);
        assert!(mods.is_empty());
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].severity, crate::Severity::Warning);
        assert!(diagnostics[0].message.contains("BrokenMod"));
        assert!(
            diagnostics[0]
                .message
                .contains(&dir.path().display().to_string()),
            "message must carry the full path so --config can distinguish Data/ vs Mods/ vs Workshop"
        );
        assert_eq!(
            diagnostics[0].source,
            crate::DiagnosticSource::FolderResolution
        );
    }

    #[test]
    fn discover_mods_silent_for_absent_and_non_dir_search_paths() {
        // Absent dirs and non-dir paths are deliberate non-errors (e.g. no
        // Workshop install): they must produce neither mods nor diagnostics.
        // The read_dir-failure Warning is not portably triggerable (would need
        // permission tricks), so it stays uncovered by unit tests.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("not_a_dir");
        std::fs::write(&file, "x").unwrap();

        let (mods, diagnostics) = discover_mods(&[dir.path().join("does_not_exist"), file], None);
        assert!(mods.is_empty());
        assert!(diagnostics.is_empty());
    }
}
