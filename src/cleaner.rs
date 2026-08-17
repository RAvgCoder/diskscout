use colored::Colorize;
use std::fs;
use std::io::{self, ErrorKind, Write};
use std::path::Path;

use crate::report::all_categories;
use crate::scanner::{Category, FoundDir, ScanResult, dir_stats, display_path, fmt_size};

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
        freed += remove_reporting(d, home, "  ");
    }

    println!("\n  {} freed.", fmt_size(freed).bold().green());
}

/// Deletes every category marked regenerable, leaving the review-carefully
/// ones untouched. One confirmation covers the whole set.
pub fn delete_safe(
    result: &ScanResult,
    home: &Path,
    assume_yes: bool,
    except: &[String],
) -> Result<(), String> {
    let candidates: Vec<Category> = all_categories()
        .filter(|c| c.safe_to_delete() && !result.dirs_by_category(*c).is_empty())
        .collect();

    // A name matching no category at all is a typo, and the caller believes it
    // held something back, so refuse rather than delete it. Naming a real
    // category this scan did not turn up already excludes nothing, which is
    // what a fixed --except set on a machine without that cache means.
    for name in except {
        if !all_categories().any(|c| &c.slug() == name) {
            let valid: Vec<String> = candidates.iter().map(|c| c.slug()).collect();
            return Err(format!(
                "unknown category '{name}'.\n  Deletable categories in this scan: {}",
                valid.join(", ")
            ));
        }
    }

    let cats: Vec<Category> = candidates
        .into_iter()
        .filter(|c| !except.contains(&c.slug()))
        .collect();

    if cats.is_empty() {
        println!("Nothing regenerable found.");
        return Ok(());
    }

    for name in except {
        println!("  {} {name}", "excluded:".yellow());
    }

    let total: u64 = cats.iter().map(|c| result.category_total(*c)).sum();

    println!();
    println!("  {}", "About to delete these categories:".bold());
    for cat in &cats {
        let dirs = result.dirs_by_category(*cat);
        println!(
            "    {:<28} {:>9}  ({} {}, --except {})",
            cat.label().cyan(),
            fmt_size(result.category_total(*cat)),
            dirs.len(),
            if dirs.len() == 1 { "dir" } else { "dirs" },
            cat.slug().dimmed()
        );
    }
    println!("    {:<28} {:>9}", "TOTAL".bold(), fmt_size(total).bold());

    if !assume_yes && !confirm(&format!("\n  Delete all of it? [{}/N] ", "y".green())) {
        println!("  Skipped.");
        return Ok(());
    }

    let mut total_freed = 0u64;
    for cat in &cats {
        println!("\n  {}", cat.label().cyan().bold());
        for d in &result.dirs_by_category(*cat) {
            total_freed += remove_reporting(d, home, "    ");
        }
    }

    println!();
    println!("  Total freed: {}", fmt_size(total_freed).bold().green());
    Ok(())
}

pub fn interactive_delete(result: &ScanResult, home: &Path) {
    let mut total_freed = 0u64;

    for cat in all_categories() {
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
        if cat.delete_contents_only() {
            println!(
                "    {}",
                "(contents are cleared; the directory itself stays)".dimmed()
            );
        }

        if !confirm("  Delete this category? [y/N] ") {
            println!("  Skipped.");
            continue;
        }

        for d in &dirs {
            total_freed += remove_reporting(d, home, "    ");
        }
    }

    println!();
    println!("  Total freed: {}", fmt_size(total_freed).bold().green());
}

/// Deletes one directory, prints the outcome, and returns the bytes reclaimed.
fn remove_reporting(dir: &FoundDir, home: &Path, indent: &str) -> u64 {
    let (freed, error) = remove(dir);
    let shown = display_path(&dir.path, home);

    match error {
        None => println!("{indent}{} {}", "deleted".red(), shown.dimmed()),
        Some(e) if freed > 0 => println!(
            "{indent}{} {} -- {} freed, {e}",
            "partial".yellow(),
            shown.dimmed(),
            fmt_size(freed)
        ),
        Some(e) => eprintln!("{indent}{} {}: {e}", "error".yellow(), dir.path.display()),
    }

    freed
}

/// A live cache can hold open handles, so a partial delete is the normal case
/// on Windows: measure what is actually left rather than trusting the size the
/// scan recorded.
fn remove(dir: &FoundDir) -> (u64, Option<String>) {
    let outcome = if dir.path.is_file() {
        fs::remove_file(&dir.path).map_err(|e| e.to_string())
    } else if dir.category.delete_contents_only() {
        clear_contents(&dir.path)
    } else {
        force_remove_dir_all(&dir.path)
    };

    let remaining = if dir.path.exists() {
        dir_stats(&dir.path).0
    } else {
        0
    };

    (dir.size.saturating_sub(remaining), outcome.err())
}

/// Empties a directory but keeps it: the OS and running apps expect paths like
/// %TEMP% and the browser cache roots to exist.
fn clear_contents(path: &Path) -> Result<(), String> {
    let entries = fs::read_dir(path).map_err(|e| e.to_string())?;
    let mut in_use = 0usize;

    for entry in entries.flatten() {
        let child = entry.path();
        let removed = if entry.file_type().is_ok_and(|t| t.is_dir()) {
            force_remove_dir_all(&child)
        } else {
            // A directory symlink reports as a non-file but needs remove_dir.
            fs::remove_file(&child)
                .or_else(|_| fs::remove_dir(&child))
                .map_err(|e| e.to_string())
        };
        if removed.is_err() {
            in_use += 1;
        }
    }

    if in_use > 0 {
        Err(format!("{in_use} entries still in use"))
    } else {
        Ok(())
    }
}

fn force_remove_dir_all(path: &Path) -> Result<(), String> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        // Read-only files are common in package caches; clearing the flag is
        // the one retry worth making before giving up.
        Err(e) if e.kind() == ErrorKind::PermissionDenied => {
            clear_readonly(path);
            fs::remove_dir_all(path).map_err(|e| e.to_string())
        }
        Err(e) => Err(e.to_string()),
    }
}

// The lint warns that clearing the flag makes a file world-writable on Unix.
// This function only exists on Windows, where it just drops FILE_ATTRIBUTE_READONLY.
#[cfg(windows)]
#[allow(clippy::permissions_set_readonly_false)]
fn clear_readonly(root: &Path) {
    use walkdir::WalkDir;

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let mut perms = meta.permissions();
        if perms.readonly() {
            perms.set_readonly(false);
            let _ = fs::set_permissions(entry.path(), perms);
        }
    }
}

// A cache can be deliberately unwritable rather than owned by someone else: Go
// drops the write bit on every module directory so a build cannot edit its own
// dependencies, and the delete then fails on the directory, not the file. Only
// the owner bit is added, so this cannot widen the mode for anyone else, and a
// denial that really is an ownership problem still fails the retry.
#[cfg(not(windows))]
fn clear_readonly(root: &Path) {
    use std::os::unix::fs::PermissionsExt;
    use walkdir::WalkDir;

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let mode = meta.permissions().mode();
        if mode & 0o200 == 0 {
            let _ = fs::set_permissions(entry.path(), fs::Permissions::from_mode(mode | 0o200));
        }
    }
}

fn confirm(prompt: &str) -> bool {
    print!("{prompt}");
    io::stdout().flush().unwrap();
    let mut input = String::new();
    io::stdin().read_line(&mut input).unwrap();
    input.trim().eq_ignore_ascii_case("y")
}

#[cfg(test)]
mod tests {
    use super::*;

    // An empty scan deletes nothing, so these reach the validation and stop.
    fn check(except: &str) -> Result<(), String> {
        delete_safe(
            &ScanResult::default(),
            Path::new("/"),
            true,
            &[except.to_owned()],
        )
    }

    #[test]
    fn except_accepts_a_category_this_scan_did_not_find() {
        assert!(check("browser-caches").is_ok());
        assert!(check("recycle-bin").is_ok());
        // Naming one that is never auto-deleted is redundant, not wrong.
        assert!(check("iphone-backups").is_ok());
    }

    #[test]
    fn except_rejects_a_name_that_is_no_category_at_all() {
        assert!(check("brwoser-caches").is_err());
        assert!(check("").is_err());
    }

    // An abandoned Instruments recording is a loose file, not a directory.
    #[test]
    fn a_file_target_is_removed_rather_than_walked() {
        let dir = tempfile::tempdir().unwrap();
        let trace = dir.path().join("instrumentsAbc.ktrace");
        fs::write(&trace, vec![0u8; 4_096]).unwrap();

        let (freed, error) = remove(&FoundDir {
            path: trace.clone(),
            category: Category::InstrumentsTraces,
            size: 4_096,
            last_modified: std::time::SystemTime::UNIX_EPOCH,
        });

        assert_eq!(error, None);
        assert_eq!(freed, 4_096);
        assert!(!trace.exists());
    }

    // Go marks every module directory read-only, so the whole category failed
    // with "Permission denied" on a tree the user owns outright.
    #[cfg(not(windows))]
    #[test]
    fn a_deliberately_unwritable_cache_is_still_removed() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("mod");
        let locked = cache.join("github.com!example/pkg@v1.0.0");
        fs::create_dir_all(&locked).unwrap();
        fs::write(locked.join("go.mod"), vec![0u8; 1_024]).unwrap();
        // Read and execute only, which is exactly how Go leaves them.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();

        let (freed, error) = remove(&FoundDir {
            path: cache.clone(),
            category: Category::GoModCache,
            size: 1_024,
            last_modified: std::time::SystemTime::UNIX_EPOCH,
        });

        assert_eq!(error, None);
        assert_eq!(freed, 1_024);
        assert!(!cache.exists());
    }

    // A contents-only category keeps the directory the OS expects to exist.
    #[test]
    fn clearing_a_live_cache_leaves_the_directory_behind() {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("Cache");
        fs::create_dir_all(cache.join("nested")).unwrap();
        fs::write(cache.join("nested").join("blob"), vec![0u8; 2_048]).unwrap();

        let (freed, error) = remove(&FoundDir {
            path: cache.clone(),
            category: Category::AppWebCache,
            size: 2_048,
            last_modified: std::time::SystemTime::UNIX_EPOCH,
        });

        assert_eq!(error, None);
        assert_eq!(freed, 2_048);
        assert!(cache.is_dir(), "the cache root must survive");
        assert_eq!(fs::read_dir(&cache).unwrap().count(), 0);
    }
}
