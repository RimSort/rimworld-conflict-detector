use crate::Severity;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
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
