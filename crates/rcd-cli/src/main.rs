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

    if resolved.mods.is_empty() && !resolved.diagnostics.is_empty() {
        // All mods failed to resolve
        for d in &resolved.diagnostics {
            eprintln!("[{}] {}", d.severity, d.message);
        }
        return 2;
    }

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

    // Merge resolution diagnostics
    report.diagnostics.extend(resolved.diagnostics);

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

    if report.has_errors() { 1 } else { 0 }
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
