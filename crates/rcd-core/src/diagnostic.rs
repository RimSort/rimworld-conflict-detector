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
    /// True when analysis is known to have been skipped or degraded.
    pub analysis_incomplete: bool,
    /// Stable identifier for a translatable explanation, when available.
    #[serde(default)]
    pub code: Option<String>,
    /// Named values used by the explanation template.
    #[serde(default)]
    pub params: std::collections::BTreeMap<String, String>,
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
        analysis_incomplete: true,
        code: Some("about_parse_failure".to_owned()),
        params: [
            ("path".to_owned(), path.display().to_string()),
            ("detail".to_owned(), err.to_string()),
        ]
        .into_iter()
        .collect(),
    }
}

#[cfg(test)]
mod structured_tests {
    use super::*;

    #[test]
    fn about_parse_failure_serializes_structured_fields_and_preserves_message() {
        let diagnostic = about_parse_failure(
            std::path::Path::new("/mods/example"),
            &"bad xml",
            Severity::Warning,
            DiagnosticSource::FolderResolution,
        );
        let value = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(
            value["message"],
            "Failed to parse mod at /mods/example: bad xml"
        );
        assert_eq!(value["code"], "about_parse_failure");
        assert_eq!(value["params"]["path"], "/mods/example");
        assert_eq!(value["params"]["detail"], "bad xml");
    }

    #[test]
    fn older_diagnostic_without_structured_fields_deserializes() {
        let value = serde_json::json!({
            "affected_mod": null, "severity": "warning", "message": "legacy",
            "source": "folder_resolution", "analysis_incomplete": true
        });
        let diagnostic: Diagnostic = serde_json::from_value(value).unwrap();
        let value = serde_json::to_value(diagnostic).unwrap();
        assert_eq!(value["code"], serde_json::Value::Null);
        assert_eq!(value["params"], serde_json::json!({}));
    }
}
