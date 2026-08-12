use colored::Colorize;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::scanner::{Category, FoundFile, LargeDir, ScanResult, display_path, fmt_age, fmt_size};

// Sections whose categories are all empty are skipped when printing, so the
// macOS and Windows blocks can both live here unconditionally.
pub(crate) const SECTION_CATS: &[(&str, &[Category])] = &[
    (
        "BUILD ARTIFACTS",
        &[
            Category::RustTarget,
            Category::NodeModules,
            Category::PythonBytecode,
            Category::PythonEnv,
            Category::NextJs,
            Category::Nuxt,
            Category::SvelteKit,
            Category::CocoaPods,
            Category::SwiftPmBuild,
            Category::DotNetBuild,
            Category::VisualStudioCache,
            Category::GradleBuild,
        ],
    ),
    (
        "MACOS & XCODE",
        &[
            Category::XcodeDerivedData,
            Category::XcTestDevices,
            Category::IosSimulators,
            Category::SimulatorRuntimes,
            Category::XcodeDeviceSupport,
            Category::XcodeCaches,
            Category::InstrumentsTraces,
            Category::MacOsCaches,
            Category::IphoneBackups,
        ],
    ),
    (
        "APP DATA & CACHES",
        &[
            Category::AppWebCache,
            Category::ContainerCaches,
            Category::ExpensiveCache,
            Category::CloudMirror,
        ],
    ),
    (
        "WINDOWS SYSTEM",
        &[
            Category::WindowsTemp,
            Category::WindowsUpdate,
            Category::WindowsOld,
            Category::RecycleBin,
            Category::CrashDumps,
            Category::WindowsCaches,
            Category::ThumbnailCache,
            Category::InstallerCache,
            Category::BrowserCache,
        ],
    ),
    (
        "PACKAGE & TOOL CACHES",
        &[
            Category::NpmCache,
            Category::YarnPnpmCache,
            Category::HomebrewCache,
            Category::UvCache,
            Category::PipCache,
            Category::NugetCache,
            Category::CargoRegistry,
            Category::GradleCache,
            Category::MavenCache,
            Category::GoModCache,
            Category::IdeCache,
        ],
    ),
    (
        "VMS, SDKS & CONTAINERS",
        &[
            Category::DockerData,
            Category::AndroidSdk,
            Category::AndroidEmulator,
        ],
    ),
];

/// Every category the report knows about, in display order -- the single list
/// the interactive cleaner walks so the two can never drift apart.
pub(crate) fn all_categories() -> impl Iterator<Item = Category> {
    SECTION_CATS
        .iter()
        .flat_map(|(_, cats)| cats.iter().copied())
}

const STALE_DAYS: u64 = 180;

pub fn print_report(result: &ScanResult, home: &Path) {
    println!();
    println!("{}", "diskscout -- storage cleanup report".bold());
    println!("{}", "─".repeat(60).dimmed());

    for (section_name, cats) in SECTION_CATS {
        let section_total: u64 = cats.iter().map(|c| result.category_total(*c)).sum();
        if section_total == 0 {
            continue;
        }

        println!();
        println!(
            "  {}  {}",
            section_name.bold().yellow(),
            fmt_size(section_total).bold()
        );

        for cat in *cats {
            let dirs = result.dirs_by_category(*cat);
            if dirs.is_empty() {
                continue;
            }
            let cat_total: u64 = dirs.iter().map(|d| d.size).sum();
            let safe_label = if cat.safe_to_delete() {
                " (safe to delete)".dimmed().to_string()
            } else {
                String::new()
            };

            println!(
                "    {:<28} {}{}",
                cat.label().cyan(),
                fmt_size(cat_total).bold(),
                safe_label
            );

            for dir in dirs.iter().take(8) {
                let age = fmt_age(dir.last_modified);
                println!(
                    "      {:>10}  {}  {}",
                    fmt_size(dir.size).green(),
                    display_path(&dir.path, home).dimmed(),
                    format!("({age})").dimmed()
                );
            }
            if dirs.len() > 8 {
                println!("      {} more...", dirs.len() - 8);
            }
        }
    }

    if !result.large_unknown_dirs.is_empty() {
        print_unknown_dirs(&result.large_unknown_dirs, home);
    }

    if !result.large_files.is_empty() {
        println!();
        println!(
            "  {}  (review before deleting)",
            "LARGE FILES".bold().yellow()
        );
        print_files(&result.large_files, home, 15);
    }

    if !result.old_files.is_empty() {
        println!();
        println!(
            "  {}  (not modified in 6+ months)",
            "OLD UNUSED FILES".bold().yellow()
        );
        print_files(&result.old_files, home, 15);
    }

    if !result.disk_images.is_empty() {
        println!();
        println!(
            "  {}  (forgotten installers and VM images)",
            "DISK IMAGES & INSTALLERS".bold().yellow()
        );
        print_files(&result.disk_images, home, 20);
    }

    if !result.reserved_files.is_empty() {
        println!();
        println!(
            "  {}  (not deletable directly)",
            "RESERVED SYSTEM FILES".bold().yellow()
        );
        for f in &result.reserved_files {
            println!(
                "      {:>10}  {}",
                fmt_size(f.size).green(),
                f.path.display().to_string().dimmed()
            );
            println!("                  {}", f.hint.dimmed());
        }
    }

    println!();
    println!("{}", "─".repeat(60).dimmed());
    println!(
        "  {}  {}",
        "TOTAL RECLAIMABLE (dirs + disk images):".bold(),
        fmt_size(result.grand_total()).bold().red()
    );

    if result.permission_errors > 0 {
        println!(
            "  {} {} directories skipped due to permission errors",
            "note:".yellow(),
            result.permission_errors
        );
    }

    println!();
    println!(
        "{}",
        "  Run with --delete to choose what to remove.".dimmed()
    );
    println!(
        "{}",
        "  Run with --delete-targets to nuke all Rust target/ dirs immediately.".dimmed()
    );
    for hint in crate::platform::extra_hints() {
        println!("{}", format!("  {hint}").dimmed());
    }
    println!();
}

fn print_unknown_dirs(dirs: &[LargeDir], home: &Path) {
    let stale_threshold = Duration::from_secs(STALE_DAYS * 86_400);

    println!();
    println!(
        "  {}  (not matched by any known pattern)",
        "UNEXPLORED LARGE DIRECTORIES".bold().yellow()
    );

    for d in dirs {
        let age = fmt_age(d.last_modified);
        let stale = SystemTime::now()
            .duration_since(d.last_modified)
            .is_ok_and(|a| a >= stale_threshold);

        let stale_tag = if stale {
            format!("  {}", "[stale]".red())
        } else {
            String::new()
        };

        println!(
            "    {}  {}  {}{}",
            fmt_size(d.size).bold().green(),
            display_path(&d.path, home).cyan(),
            format!("(last used: {age})").dimmed(),
            stale_tag
        );

        for (child, child_size) in &d.notable_children {
            let child_name = child.file_name().and_then(|n| n.to_str()).unwrap_or("?");
            println!(
                "      {:>10}  {}",
                fmt_size(*child_size).green(),
                child_name.dimmed()
            );
        }
    }
}

fn print_files(files: &[FoundFile], home: &Path, limit: usize) {
    for f in files.iter().take(limit) {
        println!(
            "      {:>10}  {}  {}",
            fmt_size(f.size).green(),
            display_path(&f.path, home).dimmed(),
            format!("({})", fmt_age(f.modified)).yellow()
        );
    }
    if files.len() > limit {
        println!("      ... and {} more", files.len() - limit);
    }
}
