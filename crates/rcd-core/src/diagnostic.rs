use crate::Severity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSource {
    XmlParsing,
    DllParsing,
    MetadataResolution,
    FolderResolution,
    ModsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub affected_mod: Option<String>,
    pub severity: Severity,
    pub message: String,
    pub source: DiagnosticSource,
}

/// One rendering for About.xml parse failures across CLI/core/FFI: full
/// Display path, consistent wording. Severity and source stay context-dependent
/// at the call sites (Error/MetadataResolution for explicitly-named paths,
/// Warning/FolderResolution for ambient directory discovery).
pub fn about_parse_failure(
    path: &std::path::Path,
    err: &impl std::fmt::Display,
    severity: crate::Severity,
    source: DiagnosticSource,
) -> Diagnostic {
    Diagnostic {
        affected_mod: None, // package_id is unknown — parsing failed
        severity,
        message: format!("Failed to parse mod at {}: {}", path.display(), err),
        source,
    }
}
