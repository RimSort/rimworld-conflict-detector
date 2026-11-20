//! Deterministic content-file enumeration with first-directory-wins precedence.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::Mod;

/// Mark Ludeon-published mods as base content.
pub fn mark_base_content(mods: &mut [Mod]) {
    for m in mods {
        if m.package_id.starts_with("ludeon.") {
            m.is_base_content = true;
        }
    }
}

/// Collect files from content directories in precedence order.
///
/// The first occurrence of each case-sensitive relative path wins. Returned
/// files are grouped by directory precedence and sorted within each directory.
pub fn collect_files_with_precedence(dirs: &[PathBuf]) -> (Vec<PathBuf>, Vec<String>) {
    let (files, warnings) = collect_files_with_precedence_structured(dirs);
    (
        files,
        warnings
            .into_iter()
            .map(|warning| warning.message)
            .collect(),
    )
}

#[derive(Debug, Clone)]
pub struct ContentWarning {
    pub message: String,
    pub code: &'static str,
    pub params: std::collections::BTreeMap<String, String>,
}

pub fn collect_files_with_precedence_structured(
    dirs: &[PathBuf],
) -> (Vec<PathBuf>, Vec<ContentWarning>) {
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    let mut warnings = Vec::new();
    for dir in dirs {
        let mut contents = Vec::new();
        if let Err(error) = collect_dir(dir, dir, &mut contents, &mut warnings) {
            warnings.push(ContentWarning {
                message: format!("Failed to enumerate {}: {error}", dir.display()),
                code: "content_enumeration_failure",
                params: [
                    ("path".into(), dir.display().to_string()),
                    ("detail".into(), error.to_string()),
                ]
                .into_iter()
                .collect(),
            });
        }
        contents.sort_by(|left, right| left.0.cmp(&right.0));
        for (relative, full_path) in contents {
            if seen.insert(relative) {
                files.push(full_path);
            }
        }
    }
    (files, warnings)
}

fn collect_dir(
    root: &Path,
    dir: &Path,
    files: &mut Vec<(String, PathBuf)>,
    warnings: &mut Vec<ContentWarning>,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_dir(root, &path, files, warnings)?;
        } else if file_type.is_symlink() {
            warnings.push(ContentWarning {
                message: format!("Skipped symlink {}", path.display()),
                code: "content_symlink_skipped",
                params: [("path".into(), path.display().to_string())]
                    .into_iter()
                    .collect(),
            });
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)
                .expect("walked path is under root")
                .to_string_lossy()
                .replace('\\', "/");
            files.push((relative, path));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn skipped_nested_symlinks_are_warned_without_hiding_normal_files() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("nested")).unwrap();
        std::fs::write(dir.path().join("nested/normal.xml"), "normal").unwrap();
        symlink("nested/normal.xml", dir.path().join("file-link.xml")).unwrap();
        symlink("nested", dir.path().join("dir-link")).unwrap();
        let (files, warnings) = collect_files_with_precedence(&[dir.path().to_owned()]);
        let (_, structured) = collect_files_with_precedence_structured(&[dir.path().to_owned()]);
        assert_eq!(structured.len(), 2);
        assert!(
            structured
                .iter()
                .all(|warning| warning.code == "content_symlink_skipped")
        );
        assert!(
            structured
                .iter()
                .any(|warning| warning.params["path"].contains("file-link.xml"))
        );
        assert_eq!(files, [dir.path().join("nested/normal.xml")]);
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().any(|w| w.contains("file-link.xml")));
        assert!(warnings.iter().any(|w| w.contains("dir-link")));
    }

    #[test]
    fn nested_duplicates_are_first_wins_case_sensitive_and_deterministic() {
        let root = tempfile::tempdir().unwrap();
        let high = root.path().join("high");
        let low = root.path().join("low");
        let case_variant = root.path().join("case_variant");
        // Separate roots preserve both spellings even on case-insensitive filesystems.
        for (base, rel, value) in [
            (&high, "Defs/nested/a.xml", "high"),
            (&high, "Defs/A.xml", "upper"),
            (&low, "Defs/nested/a.xml", "low"),
            (&case_variant, "Defs/nested/A.xml", "distinct"),
        ] {
            let file = base.join(rel);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, value).unwrap();
        }
        let (files, warnings) =
            collect_files_with_precedence(&[high.clone(), low, case_variant.clone()]);
        assert!(warnings.is_empty());
        let rels = files
            .iter()
            .map(|file| {
                file.strip_prefix(&high)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| {
                        format!(
                            "case_variant/{}",
                            file.strip_prefix(&case_variant).unwrap().display()
                        )
                    })
                    .replace('\\', "/")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            rels,
            [
                "Defs/A.xml",
                "Defs/nested/a.xml",
                "case_variant/Defs/nested/A.xml"
            ]
        );
        assert_eq!(std::fs::read_to_string(&files[1]).unwrap(), "high");
        assert_eq!(std::fs::read_to_string(&files[2]).unwrap(), "distinct");
    }
}
