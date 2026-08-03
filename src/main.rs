use clap::{Parser, Subcommand};
use std::path::PathBuf;

mod bootstrap;
mod cleaner;
mod dump;
mod platform;
mod report;
mod scanner;

use scanner::{Category, ScanConfig, Scanner};

#[derive(Debug, Parser)]
#[command(
    name = "diskscout",
    about = "Scan your disk for space hogs and help you clean them up",
    version,
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    scan: ScanArgs,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Install the diskscout binary and set up shell completions.
    ///
    ///   diskscout __bootstrap
    ///   diskscout __bootstrap --only-completions
    #[command(name = "__bootstrap", hide = true)]
    Bootstrap(bootstrap::BootstrapArgs),
}

#[derive(Debug, clap::Args)]
struct ScanArgs {
    /// Scan a specific path instead of $HOME
    #[arg(long, value_name = "DIR")]
    path: Option<PathBuf>,

    /// Prompt to delete each category after scanning
    #[arg(long)]
    delete: bool,

    /// Delete all Rust target/ directories without interactive prompts
    #[arg(long)]
    delete_targets: bool,

    /// Delete every category marked regenerable, skipping the rest
    #[arg(long)]
    delete_safe: bool,

    /// Answer yes to the --delete-safe confirmation
    #[arg(long, short = 'y')]
    yes: bool,

    /// Categories --delete-safe must leave alone, e.g. --except browser-caches
    #[arg(long, value_delimiter = ',', value_name = "CAT")]
    except: Vec<String>,

    /// Days since last modification before a file is flagged as old
    #[arg(long, default_value_t = 180)]
    old_after: u64,

    /// Minimum file size in MB before it is flagged as large
    #[arg(long, default_value_t = 100)]
    large_over: u64,

    /// Skip OS caches, browser caches, and other system directories
    #[arg(long)]
    no_system: bool,
}

fn main() {
    let cli = Cli::parse();

    if let Some(Command::Bootstrap(args)) = &cli.command {
        if let Err(e) = bootstrap::run(args) {
            eprintln!("{e}");
            std::process::exit(1);
        }
        return;
    }

    let cli = cli.scan;

    let Some(home) = platform::home_dir() else {
        eprintln!(
            "diskscout: cannot locate your home directory ({} is not set)",
            platform::HOME_VAR
        );
        std::process::exit(1);
    };

    let root = cli.path.clone().unwrap_or_else(|| home.clone());

    let config = ScanConfig {
        root,
        home: home.clone(),
        old_after_days: cli.old_after,
        large_over_mb: cli.large_over,
        include_system: !cli.no_system,
    };

    let result = Scanner::new(config).scan();
    report::print_report(&result, &home);

    let dump_path = platform::report_path();
    match dump::write_full_report(&result, &home, &dump_path) {
        Ok(()) => println!("  Full untruncated report: {}\n", dump_path.display()),
        Err(e) => eprintln!(
            "  warning: could not write full report to {}: {e}",
            dump_path.display()
        ),
    }

    if cli.delete_targets {
        cleaner::delete_category(&result, Category::RustTarget, &home);
    } else if cli.delete_safe {
        if let Err(e) = cleaner::delete_safe(&result, &home, cli.yes, &cli.except) {
            eprintln!("diskscout: {e}");
            std::process::exit(1);
        }
    } else if cli.delete {
        cleaner::interactive_delete(&result, &home);
    }
}
