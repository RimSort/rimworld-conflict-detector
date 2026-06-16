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
pub fn discover_mods(search_dirs: &[PathBuf], game_version: Option<&str>) -> Vec<Mod> {
    let mut mods = Vec::new();

    for search_dir in search_dirs {
        if !search_dir.is_dir() {
            continue;
        }

        // Read all subdirectories in the search directory
        let Ok(entries) = std::fs::read_dir(search_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Check if About/About.xml exists
            let about_xml = path.join("About/About.xml");
            if !about_xml.exists() {
                continue;
            }

            // Try to parse the mod
            if let Ok(mut mod_data) = parse_about_xml(&path) {
                resolve_mod_folders(&mut mod_data, game_version, None);
                mods.push(mod_data);
            }
        }
    }

    mods
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("tests/fixtures/mods")
    }

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
        let mods = discover_mods(&[fixtures_path()], Some("1.5"));
        assert!(mods.len() >= 2);
        assert!(mods.iter().any(|m| m.package_id == "test.mod.a"));
        assert!(mods.iter().any(|m| m.package_id == "test.mod.versioned"));
    }
}
