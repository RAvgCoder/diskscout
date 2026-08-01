use colored::Colorize;
use std::io::{self, Write};
use std::path::Path;

use crate::scanner::{Category, ScanResult, display_path, fmt_size};

pub fn delete_category(result: &ScanResult, category: Category, home: &Path) {
    let targets = result.dirs_by_category(category);

    if targets.is_empty() {
        println!("No {} found.", category.label());
        return;
    }

    let total: u64 = targets.iter().map(|d| d.size).sum();
    println!();
    println!(
        "  {} {}  ({})",
        targets.len(),
        category.label(),
        fmt_size(total).bold()
    );
    for d in &targets {
        println!("    {}", display_path(&d.path, home).dimmed());
    }

    if !confirm(&format!(
        "\nDelete all {} dirs? [{}/n] ",
        category.label(),
        "y".green()
    )) {
        println!("Skipped.");
        return;
    }

    let mut freed = 0u64;
    for d in &targets {
        match std::fs::remove_dir_all(&d.path) {
            Ok(()) => {
                freed += d.size;
                println!(
                    "  {} {}",
                    "deleted".red(),
                    display_path(&d.path, home).dimmed()
                );
            }
            Err(e) => {
                eprintln!("  {} {}: {}", "error".yellow(), d.path.display(), e);
            }
        }
    }

    println!("\n  {} freed.", fmt_size(freed).bold().green());
}

pub fn interactive_delete(result: &ScanResult, home: &Path) {
    use crate::scanner::Category::*;

    let all_cats = [
        RustTarget,
        NodeModules,
        PythonBytecode,
        PythonEnv,
        NextJs,
        Nuxt,
        SvelteKit,
        CocoaPods,
        XcodeDerivedData,
        IosSimulators,
        MacOsCaches,
        IphoneBackups,
        NpmCache,
        HomebrewCache,
        UvCache,
    ];

    let mut total_freed = 0u64;

    for cat in all_cats {
        let dirs = result.dirs_by_category(cat);
        if dirs.is_empty() {
            continue;
        }

        let cat_total: u64 = dirs.iter().map(|d| d.size).sum();
        let safe = if cat.safe_to_delete() {
            " [safe to delete]".green().to_string()
        } else {
            " [review carefully]".yellow().to_string()
        };

        println!(
            "\n  {} -- {}{}",
            cat.label().cyan().bold(),
            fmt_size(cat_total).bold(),
            safe
        );
        for d in dirs.iter().take(5) {
            println!("    {}", display_path(&d.path, home).dimmed());
        }
        if dirs.len() > 5 {
            println!("    ... and {} more", dirs.len() - 5);
        }

        if !confirm("  Delete this category? [y/N] ") {
            println!("  Skipped.");
            continue;
        }

        for d in &dirs {
            match std::fs::remove_dir_all(&d.path) {
                Ok(()) => {
                    total_freed += d.size;
                    println!(
                        "    {} {}",
                        "deleted".red(),
                        display_path(&d.path, home).dimmed()
                    );
                }
                Err(e) => {
                    eprintln!("    {} {}: {}", "error".yellow(), d.path.display(), e);
                }
            }
        }
    }

    println!();
    println!("  Total freed: {}", fmt_size(total_freed).bold().green());
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt}");
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().eq_ignore_ascii_case("y")
}
