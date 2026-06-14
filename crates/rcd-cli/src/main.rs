mod args;

use args::{Cli, Commands};
use clap::Parser;

fn main() {
    let cli = Cli::parse();

    let exit_code = match cli.command {
        Commands::Scan {
            input,
            ordered,
            game_version,
            rimworld_path,
        } => {
            eprintln!("Scan command: not yet implemented");
            2
        }
        Commands::ListMods {
            input,
            game_version,
            rimworld_path,
        } => {
            eprintln!("ListMods command: not yet implemented");
            2
        }
    };

    std::process::exit(exit_code);
}
