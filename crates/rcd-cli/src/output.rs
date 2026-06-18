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

    // Diagnostics first
    for d in &report.diagnostics {
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
    if !report.diagnostics.is_empty() {
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

    // Summary
    if use_color {
        output.push_str(&format!(
            "Summary: {} error(s), {} warning(s), {} info\n",
            report
                .summary
                .conflicts_by_severity
                .error
                .to_string()
                .red()
                .bold(),
            report
                .summary
                .conflicts_by_severity
                .warning
                .to_string()
                .yellow()
                .bold(),
            report
                .summary
                .conflicts_by_severity
                .info
                .to_string()
                .dimmed()
        ));
    } else {
        output.push_str(&format!(
            "Summary: {} error(s), {} warning(s), {} info\n",
            report.summary.conflicts_by_severity.error,
            report.summary.conflicts_by_severity.warning,
            report.summary.conflicts_by_severity.info
        ));
    }

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
}
