use std::path::{Path, PathBuf};

/// DLL file-stem prefixes to skip during assembly discovery.
/// These are framework assemblies (Harmony itself, Unity, .NET base class libraries)
/// that should not be scanned for Harmony patch attributes.
const SKIP_PREFIXES: &[&str] = &[
    "0Harmony",
    "UnityEngine",
    "Assembly-CSharp",
    "mscorlib",
    "System",
];

/// Discover mod-specific .dll assemblies in an `Assemblies/` directory.
///
/// Walks the directory (non-recursively) and returns paths to `.dll` files
/// whose file stem does not start with any of the known framework prefixes.
pub fn discover_assemblies(assemblies_dir: &Path) -> Vec<PathBuf> {
    let walker = match walkdir::WalkDir::new(assemblies_dir)
        .max_depth(1)
        .into_iter()
        .next()
    {
        // If the directory itself doesn't exist, return empty
        Some(Ok(_)) => walkdir::WalkDir::new(assemblies_dir).max_depth(1),
        _ => return Vec::new(),
    };

    walker
        .into_iter()
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().is_file())
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
        })
        .filter(|entry| {
            let stem = entry
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            !SKIP_PREFIXES.iter().any(|prefix| stem.starts_with(prefix))
        })
        .map(|entry| entry.into_path())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn skip_framework_dlls() {
        let dir = tempfile::tempdir().unwrap();

        // Create framework DLLs that should be skipped
        fs::write(dir.path().join("0Harmony.dll"), b"").unwrap();
        fs::write(dir.path().join("UnityEngine.CoreModule.dll"), b"").unwrap();
        fs::write(dir.path().join("Assembly-CSharp.dll"), b"").unwrap();
        fs::write(dir.path().join("mscorlib.dll"), b"").unwrap();
        fs::write(dir.path().join("System.Collections.dll"), b"").unwrap();

        // Create mod DLLs that should be found
        fs::write(dir.path().join("MyMod.dll"), b"").unwrap();
        fs::write(dir.path().join("SomePatches.dll"), b"").unwrap();

        let mut found = discover_assemblies(dir.path());
        found.sort();

        let names: Vec<&str> = found
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(names, vec!["MyMod.dll", "SomePatches.dll"]);
    }

    #[test]
    fn nonexistent_directory_returns_empty() {
        let result = discover_assemblies(Path::new("/nonexistent/assemblies/dir"));
        assert!(result.is_empty());
    }

    #[test]
    fn ignores_non_dll_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("readme.txt"), b"").unwrap();
        fs::write(dir.path().join("config.xml"), b"").unwrap();
        fs::write(dir.path().join("MyMod.dll"), b"").unwrap();

        let found = discover_assemblies(dir.path());
        assert_eq!(found.len(), 1);
        assert!(found[0].file_name().unwrap().to_str().unwrap() == "MyMod.dll");
    }
}
