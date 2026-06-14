use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "rcd", about = "RimWorld Conflict Detector")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    #[arg(long, default_value = "human", global = true)]
    pub format: OutputFormat,

    #[arg(long, default_value = "info", global = true)]
    pub severity: MinSeverity,

    #[arg(long, default_value = "auto", global = true)]
    pub color: ColorMode,

    #[arg(long, default_value = "severity", global = true)]
    pub group_by: GroupBy,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Scan mods for conflicts
    Scan {
        #[command(flatten)]
        input: ScanInput,

        /// Enable order-aware analysis for --mods
        #[arg(long)]
        ordered: bool,

        /// Target RimWorld version
        #[arg(long)]
        game_version: Option<String>,

        /// Path to RimWorld install
        #[arg(long)]
        rimworld_path: Option<PathBuf>,
    },
    /// List discovered mods and metadata
    ListMods {
        #[command(flatten)]
        input: ScanInput,

        /// Target RimWorld version
        #[arg(long)]
        game_version: Option<String>,

        /// Path to RimWorld install
        #[arg(long)]
        rimworld_path: Option<PathBuf>,
    },
}

#[derive(clap::Args)]
#[group(required = true, multiple = false)]
pub struct ScanInput {
    /// Read ModsConfig.xml for active mod list + load order
    #[arg(long)]
    pub config: Option<PathBuf>,

    /// Scan a mods directory
    #[arg(long)]
    pub dir: Option<PathBuf>,

    /// Check specific mods against each other (2+)
    #[arg(long, num_args = 2..)]
    pub mods: Option<Vec<PathBuf>>,
}

#[derive(Clone, ValueEnum)]
pub enum OutputFormat {
    Human,
    Json,
}

#[derive(Clone, ValueEnum)]
pub enum MinSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Clone, ValueEnum)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

#[derive(Clone, ValueEnum)]
pub enum GroupBy {
    Severity,
    Mod,
}
