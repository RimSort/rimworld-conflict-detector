//! Best-effort manual Harmony patch detection.
//!
//! Scans method bodies for string literals that look like type/method references,
//! which may indicate manual patch registration via `AccessTools.Method()` or
//! similar Harmony APIs.
//!
//! This is explicitly heuristic and best-effort. Many manual patches won't be
//! detected, and some false positives are expected. The goal is to surface hints
//! that may help identify conflicts, not to be exhaustive.

use std::path::Path;

use dotnetdll::prelude::*;

/// A hint that a method body may contain a manual Harmony patch registration.
///
/// These are best-effort: the string may not actually be a patch target, and
/// many real manual patches won't produce any hints.
#[derive(Debug, Clone)]
pub struct ManualPatchHint {
    /// The possible target type name, if a type-like string was found nearby.
    pub possible_type: Option<String>,
    /// The possible target method name.
    pub possible_method: String,
    /// The DLL filename this hint was found in.
    pub dll_name: String,
}

/// Scan a DLL's method bodies for string literals that look like they could be
/// type or method names passed to `AccessTools.Method()` or similar APIs.
///
/// Returns an empty vec if:
/// - The DLL can't be read or parsed
/// - No method bodies contain string-loading instructions
/// - The IL instruction API doesn't expose string operands in a usable way
///
/// # Heuristics
///
/// A string is considered a possible method name if:
/// - It starts with an uppercase ASCII letter
/// - It is between 2 and 80 characters long
/// - It contains no spaces or newlines
/// - It doesn't look like a path, URL, or sentence
pub fn detect_manual_patches(dll_path: &Path) -> Vec<ManualPatchHint> {
    let dll_name = dll_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.dll")
        .to_string();

    let bytes = match std::fs::read(dll_path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };

    let res = match Resolution::parse(&bytes, ReadOptions::default()) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut hints = Vec::new();

    for (type_idx, _typedef) in res.enumerate_type_definitions() {
        for (_method_idx, method) in res.enumerate_methods(type_idx) {
            let Some(body) = &method.body else {
                continue;
            };

            for instruction in &body.instructions {
                // LoadString in dotnetdll stores UTF-16 data as Vec<u16>.
                // We decode it back to a Rust String for analysis.
                if let Instruction::LoadString(utf16_data) = instruction {
                    let decoded = String::from_utf16_lossy(utf16_data);
                    if looks_like_method_name(&decoded) {
                        hints.push(ManualPatchHint {
                            possible_type: None,
                            possible_method: decoded,
                            dll_name: dll_name.clone(),
                        });
                    }
                }
            }
        }
    }

    hints
}

/// Heuristic: does this string look like it could be a .NET method name?
///
/// We're looking for strings that might be passed to `AccessTools.Method("MethodName")`
/// or similar Harmony APIs. This is deliberately conservative to reduce false positives.
fn looks_like_method_name(s: &str) -> bool {
    // Must be reasonable length
    if s.len() < 2 || s.len() > 80 {
        return false;
    }

    // Must start with an uppercase ASCII letter (method names are PascalCase in C#)
    let first = match s.chars().next() {
        Some(c) if c.is_ascii_uppercase() => c,
        _ => return false,
    };

    // Must not contain spaces, newlines, or other whitespace (rules out sentences/messages)
    if s.chars().any(|c| c.is_whitespace()) {
        return false;
    }

    // Must not look like a file path
    if s.contains('/') || s.contains('\\') {
        return false;
    }

    // Must not look like a URL
    if s.contains("://") {
        return false;
    }

    // Must not look like an XML tag
    if s.starts_with('<') || s.ends_with('>') {
        return false;
    }

    // Should contain only identifier-like characters: letters, digits, underscores, dots
    // (dots for fully-qualified names like "Verse.Pawn.Kill")
    if !s
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    {
        return false;
    }

    // Avoid single-word all-caps strings that are likely constants/enums, not method names.
    // Method names typically have mixed case (PascalCase).
    // Allow if it contains a dot (qualified name) or has mixed case.
    if s.len() > 3 && !s.contains('.') && s.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return false;
    }

    // Require at least one lowercase letter somewhere (after the first char) to look
    // like PascalCase, OR a dot for qualified names like "SomeType.SomeMethod"
    let has_lower = s[first.len_utf8()..]
        .chars()
        .any(|c| c.is_ascii_lowercase());
    let has_dot = s.contains('.');

    has_lower || has_dot
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dll_path() -> std::path::PathBuf {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        std::path::PathBuf::from(manifest_dir)
            .join("../../tests/fixtures/harmony/dlls/TestPatches.dll")
    }

    #[test]
    fn detect_manual_patches_does_not_crash_on_valid_dll() {
        // The main requirement: this should not panic or crash
        let hints = detect_manual_patches(&test_dll_path());
        // We don't assert specific results since the test DLL may or may not
        // contain strings that pass the heuristic. Just verify it runs.
        println!("Manual patch hints found: {}", hints.len());
        for hint in &hints {
            println!(
                "  possible_method={:?}, possible_type={:?}, dll={}",
                hint.possible_method, hint.possible_type, hint.dll_name
            );
        }
    }

    #[test]
    fn detect_manual_patches_returns_empty_for_nonexistent_dll() {
        let hints = detect_manual_patches(Path::new("/nonexistent/path/test.dll"));
        assert!(hints.is_empty());
    }

    #[test]
    fn heuristic_accepts_method_names() {
        assert!(looks_like_method_name("Kill"));
        assert!(looks_like_method_name("IsNullOrEmpty"));
        assert!(looks_like_method_name("GetComponent"));
        assert!(looks_like_method_name("Verse.Pawn.Kill"));
        assert!(looks_like_method_name("Do_Something"));
    }

    #[test]
    fn heuristic_rejects_non_method_strings() {
        // Sentence with spaces
        assert!(!looks_like_method_name("This is a log message"));
        // Lowercase start
        assert!(!looks_like_method_name("notAMethod"));
        // Too short
        assert!(!looks_like_method_name("X"));
        // File path
        assert!(!looks_like_method_name("Mods/SomeMod/Assemblies"));
        // URL
        assert!(!looks_like_method_name("Http://example.com"));
        // All caps constant
        assert!(!looks_like_method_name("SOME_CONSTANT"));
        // Empty
        assert!(!looks_like_method_name(""));
        // Too long
        assert!(!looks_like_method_name(&"A".repeat(81)));
    }
}
