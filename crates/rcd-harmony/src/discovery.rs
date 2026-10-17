use rcd_core::content_files::collect_files_with_precedence;
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
/// Recursively walks the directory and returns paths to `.dll` files whose
/// file stem does not start with any of the known framework prefixes.
/// Also collects I/O errors encountered during the walk.
pub fn discover_assemblies(assemblies_dir: &Path) -> (Vec<PathBuf>, Vec<String>) {
    discover_assemblies_dirs(&[assemblies_dir.to_path_buf()])
}

/// Discover mod-specific DLL assemblies from high-priority-first directories.
/// The first file for each case-sensitive relative path wins across all roots.
pub fn discover_assemblies_dirs(dirs: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let (files, errors) = collect_files_with_precedence(dirs);
    let paths = files
        .into_iter()
        .filter(|path| {
            if !path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("dll"))
            {
                return false;
            }

            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            !SKIP_PREFIXES.iter().any(|prefix| stem.starts_with(prefix))
        })
        .collect();
    (paths, errors)
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

        let (mut found, errors) = discover_assemblies(dir.path());
        assert!(errors.is_empty());
        found.sort();

        let names: Vec<&str> = found
            .iter()
            .filter_map(|p| p.file_name()?.to_str())
            .collect();
        assert_eq!(names, vec!["MyMod.dll", "SomePatches.dll"]);
    }

    #[test]
    fn discover_assemblies_dirs_applies_recursive_relative_path_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let high = temp.path().join("high");
        let low = temp.path().join("low");
        fs::create_dir_all(high.join("nested")).unwrap();
        fs::create_dir_all(low.join("nested")).unwrap();
        fs::write(high.join("nested/Duplicate.dll"), b"high").unwrap();
        fs::write(low.join("nested/Duplicate.dll"), b"low").unwrap();
        fs::write(low.join("nested/Unique.dll"), b"unique").unwrap();
        fs::write(low.join("nested/readme.txt"), b"ignored").unwrap();

        let (found, errors) = discover_assemblies_dirs(&[high.clone(), low.clone()]);
        assert!(errors.is_empty(), "unexpected discovery errors: {errors:?}");
        let relative: Vec<_> = found
            .iter()
            .map(|path| {
                path.strip_prefix(&high)
                    .map(|relative| format!("high/{}", relative.display()))
                    .unwrap_or_else(|_| {
                        format!("low/{}", path.strip_prefix(&low).unwrap().display())
                    })
            })
            .collect();
        assert_eq!(
            relative,
            vec!["high/nested/Duplicate.dll", "low/nested/Unique.dll"]
        );
    }

    #[test]
    fn nonexistent_directory_returns_empty() {
        let (result, _errors) = discover_assemblies(Path::new("/nonexistent/assemblies/dir"));
        assert!(result.is_empty());
    }

    #[test]
    fn ignores_non_dll_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("readme.txt"), b"").unwrap();
        fs::write(dir.path().join("config.xml"), b"").unwrap();
        fs::write(dir.path().join("MyMod.dll"), b"").unwrap();

        let (found, errors) = discover_assemblies(dir.path());
        assert!(errors.is_empty());
        assert_eq!(found.len(), 1);
        assert!(found[0].file_name().unwrap().to_str().unwrap() == "MyMod.dll");
    }

    #[test]
    fn discover_assemblies_returns_errors_vec() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("MyMod.dll"), b"").unwrap();

        let (found, errors) = discover_assemblies(dir.path());
        assert_eq!(found.len(), 1);
        assert!(errors.is_empty());
    }
}
