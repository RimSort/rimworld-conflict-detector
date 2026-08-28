use crate::diagnostic::Diagnostic;
use crate::{Conflict, ModSummary, Severity};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportMetadata {
    pub timestamp: String,
    pub game_version: Option<String>,
    pub order_aware: bool,
    pub mods_scanned: Vec<ModSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeverityCounts {
    pub error: u32,
    pub warning: u32,
    pub info: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportSummary {
    pub total_mods: u32,
    pub mods_with_conflicts: u32,
    pub conflicts_by_severity: SeverityCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Report {
    pub schema_version: u32,
    pub metadata: ReportMetadata,
    pub conflicts: Vec<Conflict>,
    pub diagnostics: Vec<Diagnostic>,
    pub summary: ReportSummary,
}

impl Report {
    pub fn build(
        metadata: ReportMetadata,
        conflicts: Vec<Conflict>,
        diagnostics: Vec<Diagnostic>,
    ) -> Self {
        let mut error_count = 0u32;
        let mut warning_count = 0u32;
        let mut info_count = 0u32;
        let mut mods_with_conflicts: HashSet<String> = HashSet::new();

        for conflict in &conflicts {
            match conflict.severity() {
                Severity::Error => error_count += 1,
                Severity::Warning => warning_count += 1,
                Severity::Info => info_count += 1,
            }
            for m in conflict.involved_mods() {
                mods_with_conflicts.insert(m.package_id.clone());
            }
        }

        let total_mods = metadata.mods_scanned.len() as u32;

        Report {
            schema_version: 1,
            metadata,
            summary: ReportSummary {
                total_mods,
                mods_with_conflicts: mods_with_conflicts.len() as u32,
                conflicts_by_severity: SeverityCounts {
                    error: error_count,
                    warning: warning_count,
                    info: info_count,
                },
            },
            conflicts,
            diagnostics,
        }
    }

    pub fn has_errors(&self) -> bool {
        self.summary.conflicts_by_severity.error > 0
    }

    /// True if any diagnostic is Error-severity (resolution/parse failures),
    /// independent of conflict severities. Drives CLI exit code 3.
    pub fn has_error_diagnostics(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }

    pub fn conflicts_at_or_above(&self, min_severity: Severity) -> Vec<&Conflict> {
        self.conflicts
            .iter()
            .filter(|c| c.severity() >= min_severity)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conflict::*;
    use crate::testutil::test_mod_summary;

    fn test_metadata() -> ReportMetadata {
        ReportMetadata {
            timestamp: "2026-06-14T00:00:00Z".into(),
            game_version: Some("1.5".into()),
            order_aware: false,
            mods_scanned: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
        }
    }

    #[test]
    fn report_counts_severities() {
        let conflicts = vec![
            Conflict::Def(DefConflict {
                target: DefTarget {
                    def_type: "ThingDef".into(),
                    def_name: "A".into(),
                },
                mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
                mechanism: DefConflictMechanism::DirectReplacement {
                    divergent_fields: vec!["label".into()],
                },
                winner: None,
                condition: None,
            }),
            Conflict::Def(DefConflict {
                target: DefTarget {
                    def_type: "ThingDef".into(),
                    def_name: "B".into(),
                },
                mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
                mechanism: DefConflictMechanism::XPathOverlap {
                    xpath_a: "a".into(),
                    xpath_b: "b".into(),
                    wildcard: false,
                },
                winner: None,
                condition: None,
            }),
        ];

        let report = Report::build(test_metadata(), conflicts, vec![]);
        assert_eq!(report.summary.conflicts_by_severity.error, 1);
        assert_eq!(report.summary.conflicts_by_severity.warning, 1);
        assert!(report.has_errors());
    }

    #[test]
    fn has_error_diagnostics_ignores_warnings_and_conflicts() {
        let diag = |sev: crate::Severity| crate::Diagnostic {
            affected_mod: None,
            severity: sev,
            message: "m".into(),
            source: crate::DiagnosticSource::MetadataResolution,
        };
        let meta = || ReportMetadata {
            timestamp: "t".into(),
            game_version: None,
            order_aware: false,
            mods_scanned: vec![],
        };
        let r = Report::build(
            meta(),
            vec![],
            vec![diag(crate::Severity::Warning), diag(crate::Severity::Info)],
        );
        assert!(!r.has_error_diagnostics());
        let r = Report::build(meta(), vec![], vec![diag(crate::Severity::Error)]);
        assert!(r.has_error_diagnostics());

        // Error-severity CONFLICTS must not leak into has_error_diagnostics:
        // the two channels are independent (drives exit 1 vs exit 3).
        let conflict_report = Report::build(
            meta(),
            vec![Conflict::Def(DefConflict {
                target: DefTarget {
                    def_type: "ThingDef".into(),
                    def_name: "A".into(),
                },
                mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
                mechanism: DefConflictMechanism::DirectReplacement {
                    divergent_fields: vec!["label".into()],
                },
                winner: None,
                condition: None,
            })],
            vec![diag(crate::Severity::Warning)],
        );
        assert!(conflict_report.has_errors());
        assert!(!conflict_report.has_error_diagnostics());
    }

    #[test]
    fn report_json_roundtrip() {
        let report = Report::build(test_metadata(), vec![], vec![]);
        let json = serde_json::to_string_pretty(&report).unwrap();
        let parsed: Report = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.schema_version, 1);
    }

    #[test]
    fn report_filters_by_severity() {
        let conflicts = vec![
            Conflict::Def(DefConflict {
                target: DefTarget {
                    def_type: "ThingDef".into(),
                    def_name: "A".into(),
                },
                mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
                mechanism: DefConflictMechanism::DirectReplacement {
                    divergent_fields: vec!["label".into()],
                },
                winner: None,
                condition: None,
            }),
            Conflict::Def(DefConflict {
                target: DefTarget {
                    def_type: "ThingDef".into(),
                    def_name: "B".into(),
                },
                mods: vec![test_mod_summary("mod.a"), test_mod_summary("mod.b")],
                mechanism: DefConflictMechanism::XPathOverlap {
                    xpath_a: "a".into(),
                    xpath_b: "b".into(),
                    wildcard: false,
                },
                winner: None,
                condition: None,
            }),
        ];

        let report = Report::build(test_metadata(), conflicts, vec![]);
        let errors_only = report.conflicts_at_or_above(Severity::Error);
        assert_eq!(errors_only.len(), 1);
    }
}
