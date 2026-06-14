mod args;
mod resolve;

use args::{Cli, Commands, MinSeverity, OutputFormat};
use clap::Parser;
use rcd_core::report::ReportMetadata;
use rcd_core::{Engine, Severity};

fn main() {
    let cli = Cli::parse();
    let format = cli.format;
    let severity = cli.severity;

    let exit_code = match cli.command {
        Commands::Scan {
            input,
            ordered,
            game_version,
            rimworld_path,
        } => run_scan(
            &format,
            &severity,
            input,
            ordered,
            game_version,
            rimworld_path,
        ),
        Commands::ListMods {
            input,
            game_version,
            rimworld_path,
        } => {
            let _ = (input, game_version, rimworld_path);
            eprintln!("ListMods command: not yet implemented");
            2
        }
    };

    std::process::exit(exit_code);
}

fn run_scan(
    format: &OutputFormat,
    severity_filter: &MinSeverity,
    input: args::ScanInput,
    ordered: bool,
    game_version: Option<String>,
    rimworld_path: Option<std::path::PathBuf>,
) -> i32 {
    // Resolve mods based on input mode
    let resolved = if let Some(dir) = input.dir {
        resolve::resolve_from_dir(&dir, game_version.as_deref())
    } else if let Some(mod_paths) = input.mods {
        resolve::resolve_from_mod_paths(&mod_paths, game_version.as_deref(), ordered)
    } else if let Some(config_path) = input.config {
        let rw_path = rimworld_path.unwrap_or_else(|| {
            eprintln!("Error: --rimworld-path is required with --config");
            std::process::exit(2);
        });
        resolve::resolve_from_config(&config_path, &rw_path, game_version.as_deref())
    } else {
        eprintln!("Error: no input mode specified");
        return 2;
    };

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
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        }
        OutputFormat::Human => {
            // Print diagnostics first
            for d in &report.diagnostics {
                eprintln!(
                    "[{}] {}: {}",
                    d.severity,
                    d.affected_mod.as_deref().unwrap_or("unknown"),
                    d.message
                );
            }

            // Print conflicts
            let filtered = report.conflicts_at_or_above(min_severity);
            if filtered.is_empty() {
                println!("No conflicts found at {} level or above.", min_severity);
            } else {
                println!("Found {} conflict(s):\n", filtered.len());
                for conflict in &filtered {
                    let severity = conflict.severity();
                    match conflict {
                        rcd_core::Conflict::Def(dc) => {
                            println!("[{:?}] {} -- {:?}", severity, dc.target, dc.mechanism);
                            println!(
                                "  Mods: {}",
                                dc.mods
                                    .iter()
                                    .map(|m| m.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            );
                            if let Some(winner) = &dc.winner {
                                println!("  Winner: {}", winner);
                            }
                            if let Some(cond) = &dc.condition {
                                println!("  Condition: {}", cond);
                            }
                            println!();
                        }
                        rcd_core::Conflict::Harmony(hc) => {
                            println!("[{:?}] {}", severity, hc.target);
                            for p in &hc.patches {
                                println!(
                                    "  {} ({:?}) from {} [{}] priority={}",
                                    p.mod_summary.name,
                                    p.patch_kind,
                                    p.dll_name,
                                    if p.returns_bool { "returns bool" } else { "" },
                                    p.priority
                                );
                            }
                            if hc.short_circuit_risk {
                                println!(
                                    "  WARNING: Short-circuit risk: prefix may skip original method"
                                );
                            }
                            println!();
                        }
                    }
                }
            }

            // Print summary
            println!(
                "Summary: {} error(s), {} warning(s), {} info",
                report.summary.conflicts_by_severity.error,
                report.summary.conflicts_by_severity.warning,
                report.summary.conflicts_by_severity.info
            );
        }
    };

    if report.has_errors() { 1 } else { 0 }
}
