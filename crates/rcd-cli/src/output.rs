use rcd_core::conflict::*;
use rcd_core::{Conflict, Report, Severity};

pub fn format_json(report: &Report) -> String {
    serde_json::to_string_pretty(report).unwrap()
}

pub fn format_mods_list(mods: &[rcd_core::Mod], format: &crate::args::OutputFormat) -> String {
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
                        if m.is_base_content { " [base]" } else { "" },
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

pub fn format_human(report: &Report, min_severity: Severity) -> String {
    let mut output = String::new();

    // Diagnostics first
    for d in &report.diagnostics {
        output.push_str(&format!(
            "[{}] {}: {}\n",
            d.severity,
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
        for conflict in &filtered {
            output.push_str(&format_conflict(conflict));
            output.push('\n');
        }
    }

    // Summary
    output.push_str(&format!(
        "Summary: {} error(s), {} warning(s), {} info\n",
        report.summary.conflicts_by_severity.error,
        report.summary.conflicts_by_severity.warning,
        report.summary.conflicts_by_severity.info
    ));

    output
}

fn format_conflict(conflict: &Conflict) -> String {
    let severity = conflict.severity();
    let severity_tag = match severity {
        Severity::Error => "ERROR",
        Severity::Warning => "WARN",
        Severity::Info => "INFO",
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
