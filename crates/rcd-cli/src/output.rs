use std::collections::HashMap;

use owo_colors::OwoColorize;
use rcd_core::conflict::*;
use rcd_core::{Conflict, Report, Severity};

use crate::args::GroupBy;

pub fn format_json(report: &Report) -> String {
    serde_json::to_string_pretty(report).unwrap()
}

pub fn format_mods_list(
    mods: &[rcd_core::Mod],
    format: &crate::args::OutputFormat,
    use_color: bool,
) -> String {
    match format {
        crate::args::OutputFormat::Json => {
            let summaries: Vec<rcd_core::ModSummary> =
                mods.iter().map(rcd_core::ModSummary::from).collect();
            serde_json::to_string_pretty(&summaries).unwrap()
        }
        crate::args::OutputFormat::Human => {
            if mods.is_empty() {
                return "No mods found.\n".to_string();
            }
            mods.iter()
                .enumerate()
                .map(|(i, m)| {
                    format!(
                        "{:>3}. {} ({}){}\n     Versions: {}  Deps: {}",
                        i + 1,
                        m.name,
                        m.package_id,
                        if m.is_base_content {
                            if use_color {
                                format!(" {}", "[base]".dimmed())
                            } else {
                                " [base]".to_string()
                            }
                        } else {
                            String::new()
                        },
                        if m.supported_versions.is_empty() {
                            "any".into()
                        } else {
                            m.supported_versions.join(", ")
                        },
                        if m.dependencies.is_empty() {
                            "none".into()
                        } else {
                            m.dependencies.join(", ")
                        },
                    )
                })
                .collect::<Vec<_>>()
                .join("\n\n")
        }
    }
}

pub fn format_human(
    report: &Report,
    min_severity: Severity,
    group_by: &GroupBy,
    use_color: bool,
) -> String {
    let mut output = String::new();

    // Diagnostics first, filtered by min_severity for consistency with the
    // conflict list below. JSON output stays complete (machine consumers get
    // everything); this filter applies to human output only — see README.
    let shown: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.severity >= min_severity)
        .collect();
    for d in &shown {
        let sev_display = if use_color {
            match d.severity {
                Severity::Error => format!("{}", d.severity).red().bold().to_string(),
                Severity::Warning => format!("{}", d.severity).yellow().bold().to_string(),
                Severity::Info => format!("{}", d.severity).dimmed().to_string(),
            }
        } else {
            format!("{}", d.severity)
        };
        output.push_str(&format!(
            "[{}] {}: {}\n",
            sev_display,
            d.affected_mod.as_deref().unwrap_or("unknown"),
            d.message
        ));
    }
    if !shown.is_empty() {
        output.push('\n');
    }

    // Filter conflicts
    let filtered = report.conflicts_at_or_above(min_severity);

    if filtered.is_empty() {
        output.push_str(&format!(
            "No conflicts found at {} level or above.\n",
            min_severity
        ));
    } else {
        output.push_str(&format!("Found {} conflict(s):\n\n", filtered.len()));

        match group_by {
            GroupBy::Severity => {
                for &sev in &[Severity::Error, Severity::Warning, Severity::Info] {
                    if sev < min_severity {
                        continue;
                    }
                    let group: Vec<_> = filtered.iter().filter(|c| c.severity() == sev).collect();
                    if group.is_empty() {
                        continue;
                    }
                    let label = match sev {
                        Severity::Error => "Errors",
                        Severity::Warning => "Warnings",
                        Severity::Info => "Info",
                    };
                    if use_color {
                        output.push_str(&format!("── {} ({}) ──\n\n", label.bold(), group.len()));
                    } else {
                        output.push_str(&format!("── {} ({}) ──\n\n", label, group.len()));
                    }
                    for conflict in group {
                        output.push_str(&format_conflict(conflict, use_color));
                        output.push('\n');
                    }
                }
            }
            GroupBy::Mod => {
                let mut by_mod: HashMap<String, Vec<&Conflict>> = HashMap::new();
                for conflict in &filtered {
                    for m in conflict.involved_mods() {
                        by_mod.entry(m.name.clone()).or_default().push(conflict);
                    }
                }
                let mut mod_names: Vec<_> = by_mod.keys().cloned().collect();
                mod_names.sort();
                for mod_name in mod_names {
                    let conflicts = &by_mod[&mod_name];
                    if use_color {
                        output.push_str(&format!(
                            "── {} ({} conflict{}) ──\n\n",
                            mod_name.bold(),
                            conflicts.len(),
                            if conflicts.len() == 1 { "" } else { "s" }
                        ));
                    } else {
                        output.push_str(&format!(
                            "── {} ({} conflict{}) ──\n\n",
                            mod_name,
                            conflicts.len(),
                            if conflicts.len() == 1 { "" } else { "s" }
                        ));
                    }
                    for conflict in conflicts {
                        output.push_str(&format_conflict(conflict, use_color));
                        output.push('\n');
                    }
                }
            }
        }
    }

    // Summary: conflict counts (byte-identical to the prior format) plus,
    // when any exist, diagnostic counts — so an exit-3 report no longer
    // prints "0 error(s)" while a mod failed to load.
    let (ce, cw, ci) = (
        report.summary.conflicts_by_severity.error,
        report.summary.conflicts_by_severity.warning,
        report.summary.conflicts_by_severity.info,
    );
    let mut summary = if use_color {
        format!(
            "Summary: {} error(s), {} warning(s), {} info",
            ce.to_string().red().bold(),
            cw.to_string().yellow().bold(),
            ci.to_string().dimmed(),
        )
    } else {
        format!("Summary: {} error(s), {} warning(s), {} info", ce, cw, ci)
    };
    if !report.diagnostics.is_empty() {
        let de = report
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count();
        let dw = report
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count();
        let di = report
            .diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Info)
            .count();
        let (des, dws, dis) = if use_color {
            (
                de.to_string().red().bold().to_string(),
                dw.to_string().yellow().bold().to_string(),
                di.to_string().dimmed().to_string(),
            )
        } else {
            (de.to_string(), dw.to_string(), di.to_string())
        };
        summary.push_str(&format!(
            " | {} error diagnostic(s), {} warning diagnostic(s)",
            des, dws
        ));
        if di > 0 {
            summary.push_str(&format!(", {} info diagnostic(s)", dis));
        }
    }
    summary.push('\n');
    output.push_str(&summary);

    output
}

fn format_conflict(conflict: &Conflict, use_color: bool) -> String {
    let severity = conflict.severity();
    let severity_tag = if use_color {
        match severity {
            Severity::Error => "ERROR".red().bold().to_string(),
            Severity::Warning => "WARN".yellow().bold().to_string(),
            Severity::Info => "INFO".dimmed().to_string(),
        }
    } else {
        match severity {
            Severity::Error => "ERROR".to_string(),
            Severity::Warning => "WARN".to_string(),
            Severity::Info => "INFO".to_string(),
        }
    };

    let mut out = String::new();

    match conflict {
        Conflict::Def(dc) => {
            let mechanism = match &dc.mechanism {
                DefConflictMechanism::DirectReplacement { divergent_fields } => {
                    format!(
                        "direct replacement (divergent: {})",
                        divergent_fields.join(", ")
                    )
                }
                DefConflictMechanism::XPathOverlap { .. } => "XPath overlap".to_string(),
                DefConflictMechanism::XPathOnReplacedDef { replaced_by, .. } => {
                    format!("XPath patch on def replaced by {}", replaced_by)
                }
                DefConflictMechanism::XPathSameDefNonOverlap { .. } => {
                    "same def, non-overlapping XPath patches".to_string()
                }
            };
            out.push_str(&format!(
                "[{}] {} — {}\n",
                severity_tag, dc.target, mechanism
            ));
            out.push_str(&format!(
                "  Mods: {}\n",
                dc.mods
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            if let Some(winner) = &dc.winner {
                out.push_str(&format!("  Winner: {}\n", winner));
            }
            if let Some(cond) = &dc.condition {
                out.push_str(&format!("  Condition: {}\n", cond));
            }
        }
        Conflict::Harmony(hc) => {
            out.push_str(&format!(
                "[{}] {} — Harmony conflict\n",
                severity_tag, hc.target
            ));
            for p in &hc.patches {
                let ret_info = if p.returns_bool {
                    " [returns bool]"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "  {:?} from {} ({}) priority={}{}\n",
                    p.patch_kind, p.mod_summary.name, p.dll_name, p.priority, ret_info
                ));
            }
            if hc.short_circuit_risk {
                out.push_str("  Warning: prefix may skip original method\n");
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcd_core::Report;
    use rcd_core::report::ReportMetadata;

    fn empty_report() -> Report {
        Report {
            schema_version: 1,
            metadata: ReportMetadata {
                timestamp: String::new(),
                game_version: None,
                order_aware: false,
                mods_scanned: vec![],
            },
            conflicts: vec![],
            diagnostics: vec![],
            summary: rcd_core::report::ReportSummary {
                total_mods: 0,
                mods_with_conflicts: 0,
                conflicts_by_severity: rcd_core::report::SeverityCounts {
                    error: 0,
                    warning: 0,
                    info: 0,
                },
            },
        }
    }

    #[test]
    fn format_human_no_color_has_no_ansi_escapes() {
        let report = empty_report();
        let output = format_human(&report, Severity::Info, &GroupBy::Severity, false);
        assert!(
            !output.contains('\x1b'),
            "plain output should not contain ANSI escapes"
        );
    }

    #[test]
    fn format_human_with_color_summary_has_ansi_escapes() {
        // When there are conflicts, the summary line should have colored counts
        let report = empty_report();
        let output = format_human(&report, Severity::Info, &GroupBy::Severity, true);
        // Even with 0 conflicts, the summary still prints — it should contain escapes for the counts
        assert!(
            output.contains('\x1b'),
            "colored output should contain ANSI escapes"
        );
    }

    fn diag(sev: Severity, msg: &str) -> rcd_core::Diagnostic {
        rcd_core::Diagnostic {
            affected_mod: Some("test.mod".into()),
            severity: sev,
            message: msg.into(),
            source: rcd_core::DiagnosticSource::XmlParsing,
        }
    }

    #[test]
    fn format_human_filters_diagnostics_by_severity() {
        let mut report = empty_report();
        report.diagnostics = vec![
            diag(Severity::Warning, "warn diag"),
            diag(Severity::Error, "err diag"),
        ];
        let out = format_human(&report, Severity::Error, &GroupBy::Severity, false);
        assert!(
            out.contains("err diag"),
            "Error diagnostic must show at --severity error"
        );
        assert!(
            !out.contains("warn diag"),
            "Warning diagnostic must be hidden at --severity error"
        );
    }

    #[test]
    fn format_human_summary_counts_diagnostics() {
        let mut report = empty_report();
        report.diagnostics = vec![
            diag(Severity::Error, "e"),
            diag(Severity::Warning, "w1"),
            diag(Severity::Warning, "w2"),
        ];
        let out = format_human(&report, Severity::Info, &GroupBy::Severity, false);
        assert!(
            out.contains("1 error diagnostic(s)"),
            "summary must count error diagnostics: {out}"
        );
        assert!(
            out.contains("2 warning diagnostic(s)"),
            "summary must count warning diagnostics: {out}"
        );
        assert!(
            !out.contains("info diagnostic(s)"),
            "zero info diagnostics are omitted: {out}"
        );
    }

    #[test]
    fn format_human_summary_unchanged_without_diagnostics() {
        let report = empty_report();
        let out = format_human(&report, Severity::Info, &GroupBy::Severity, false);
        assert!(
            out.contains("Summary: 0 error(s), 0 warning(s), 0 info\n"),
            "conflict summary must stay byte-identical when there are no diagnostics"
        );
        assert!(
            !out.contains("diagnostic(s)"),
            "no diagnostic clause when diagnostics are empty"
        );
    }
}
