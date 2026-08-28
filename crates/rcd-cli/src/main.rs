mod args;
mod output;
mod resolve;

use std::io::IsTerminal;

use args::{Cli, Commands, MinSeverity, OutputFormat};
use clap::Parser;
use rcd_core::report::ReportMetadata;
use rcd_core::{Engine, Severity};

fn main() {
    let cli = Cli::parse();
    let format = cli.format;
    let severity = cli.severity;
    let group_by = cli.group_by;

    let use_color = match cli.color {
        args::ColorMode::Auto => std::io::stdout().is_terminal(),
        args::ColorMode::Always => true,
        args::ColorMode::Never => false,
    };

    let exit_code = match cli.command {
        Commands::Scan {
            input,
            ordered,
            game_version,
            rimworld_path,
        } => run_scan(
            &format,
            &severity,
            &group_by,
            input,
            ordered,
            game_version,
            rimworld_path,
            use_color,
        ),
        Commands::ListMods {
            input,
            game_version,
            rimworld_path,
        } => run_list_mods(&format, input, game_version, rimworld_path, use_color),
    };

    std::process::exit(exit_code);
}

/// Shared helper to resolve mods based on input mode.
fn resolve_mods(
    input: args::ScanInput,
    ordered: bool,
    game_version: Option<&str>,
    rimworld_path: Option<&std::path::Path>,
) -> resolve::ResolvedMods {
    if let Some(dir) = input.dir {
        resolve::resolve_from_dir(&dir, game_version)
    } else if let Some(mod_paths) = input.mods {
        resolve::resolve_from_mod_paths(&mod_paths, game_version, ordered)
    } else if let Some(config_path) = input.config {
        let rw_path = rimworld_path.unwrap_or_else(|| {
            eprintln!("Error: --rimworld-path is required with --config");
            std::process::exit(2);
        });
        resolve::resolve_from_config(&config_path, rw_path, game_version)
    } else {
        eprintln!("Error: no input mode specified");
        std::process::exit(2);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_scan(
    format: &OutputFormat,
    severity_filter: &MinSeverity,
    group_by: &args::GroupBy,
    input: args::ScanInput,
    ordered: bool,
    game_version: Option<String>,
    rimworld_path: Option<std::path::PathBuf>,
    use_color: bool,
) -> i32 {
    // Resolve mods based on input mode
    let resolved = resolve_mods(
        input,
        ordered,
        game_version.as_deref(),
        rimworld_path.as_deref(),
    );

    // Set up detection engine
    let mut engine = Engine::new();
    engine.add_detector(Box::new(rcd_xml::XmlDetector));
    engine.add_detector(Box::new(rcd_harmony::HarmonyDetector));

    let metadata = ReportMetadata {
        timestamp: chrono::Utc::now().to_rfc3339(),
        game_version,
        order_aware: resolved.load_order.is_some(),
        mods_scanned: resolved
            .mods
            .iter()
            .map(rcd_core::ModSummary::from)
            .collect(),
    };

    let mut report = engine.analyze(&resolved.mods, resolved.load_order.as_ref(), metadata, None);

    // Merge resolution diagnostics FIRST so they precede detector output,
    // matching the FFI boundary's splice ordering.
    report.diagnostics.splice(0..0, resolved.diagnostics);

    // Output based on format
    let min_severity = match severity_filter {
        MinSeverity::Error => Severity::Error,
        MinSeverity::Warning => Severity::Warning,
        MinSeverity::Info => Severity::Info,
    };

    match format {
        OutputFormat::Json => {
            println!("{}", output::format_json(&report));
        }
        OutputFormat::Human => {
            print!(
                "{}",
                output::format_human(&report, min_severity, group_by, use_color)
            );
        }
    }

    exit_code_for(&report)
}

/// Exit-code ladder, isolated for automated coverage: 2 (total resolution
/// failure: no mods scanned AND diagnostics exist) beats 1 (conflict Errors)
/// beats 3 (Error-severity diagnostics) beats 0 (clean). An empty scan with
/// NO diagnostics (e.g. --dir on an existing but empty directory) is 0; a
/// nonexistent --dir yields an Error diagnostic and therefore exits 2.
fn exit_code_for(report: &rcd_core::Report) -> i32 {
    if report.metadata.mods_scanned.is_empty() && !report.diagnostics.is_empty() {
        2 // total resolution failure — machine-readable diagnostics already on stdout
    } else if report.has_errors() {
        1
    } else if report.has_error_diagnostics() {
        3
    } else {
        0
    }
}

fn run_list_mods(
    format: &OutputFormat,
    input: args::ScanInput,
    game_version: Option<String>,
    rimworld_path: Option<std::path::PathBuf>,
    use_color: bool,
) -> i32 {
    // Resolve mods - list-mods doesn't use ordered parameter
    let resolved = resolve_mods(
        input,
        false,
        game_version.as_deref(),
        rimworld_path.as_deref(),
    );

    // Surface discovery diagnostics (e.g. unparseable About.xml) — they
    // explain why mods may be missing, including when none were found.
    for d in &resolved.diagnostics {
        eprintln!("[{}] {}", d.severity, d.message);
    }

    if resolved.mods.is_empty() {
        eprintln!("No mods found.");
        return 2;
    }

    println!(
        "{}",
        output::format_mods_list(&resolved.mods, format, use_color)
    );
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcd_core::{
        DefConflict, DefConflictMechanism, DefTarget, Diagnostic, DiagnosticSource, ModSummary,
        Report,
    };

    fn summary(id: &str) -> ModSummary {
        ModSummary {
            package_id: id.into(),
            name: id.into(),
            author: None,
            is_base_content: false,
        }
    }

    fn diag(sev: Severity) -> Diagnostic {
        Diagnostic {
            affected_mod: None,
            severity: sev,
            message: "m".into(),
            source: DiagnosticSource::MetadataResolution,
        }
    }

    fn meta(mods: Vec<ModSummary>) -> ReportMetadata {
        ReportMetadata {
            timestamp: "t".into(),
            game_version: None,
            order_aware: false,
            mods_scanned: mods,
        }
    }

    fn error_conflict() -> rcd_core::Conflict {
        rcd_core::Conflict::Def(DefConflict {
            target: DefTarget {
                def_type: "ThingDef".into(),
                def_name: "A".into(),
            },
            mods: vec![summary("mod.a"), summary("mod.b")],
            mechanism: DefConflictMechanism::DirectReplacement {
                divergent_fields: vec!["label".into()],
            },
            winner: None,
            condition: None,
        })
    }

    #[test]
    fn exit_ladder_all_four_arms() {
        let clean = Report::build(
            meta(vec![summary("mod.a")]),
            vec![],
            vec![diag(Severity::Warning)],
        );
        assert_eq!(exit_code_for(&clean), 0);

        let conflict = Report::build(meta(vec![summary("mod.a")]), vec![error_conflict()], vec![]);
        assert_eq!(exit_code_for(&conflict), 1);

        let total = Report::build(meta(vec![]), vec![], vec![diag(Severity::Warning)]);
        assert_eq!(exit_code_for(&total), 2);

        let err_diag = Report::build(
            meta(vec![summary("mod.a")]),
            vec![],
            vec![diag(Severity::Error)],
        );
        assert_eq!(exit_code_for(&err_diag), 3);
    }

    #[test]
    fn exit_ladder_precedence() {
        // 2 beats 1: empty mods + conflict Errors (conflicts on zero scanned
        // mods is unreachable in practice, but the ladder must not depend on
        // that assumption).
        let two_beats_one = Report::build(
            meta(vec![]),
            vec![error_conflict()],
            vec![diag(Severity::Error)],
        );
        assert_eq!(exit_code_for(&two_beats_one), 2);

        // 1 beats 3: conflict Errors AND Error diagnostics → conflict wins.
        let one_beats_three = Report::build(
            meta(vec![summary("mod.a")]),
            vec![error_conflict()],
            vec![diag(Severity::Error)],
        );
        assert_eq!(exit_code_for(&one_beats_three), 1);

        // Empty scan with NO diagnostics (existing but empty --dir) is
        // clean, not a "total failure"; a nonexistent --dir has diagnostics
        // and is covered by resolve_from_dir_reports_nonexistent_directory_as_error.
        let empty_clean = Report::build(meta(vec![]), vec![], vec![]);
        assert_eq!(exit_code_for(&empty_clean), 0);
    }
}
