use std::fmt::Write as _;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crate::report::SECTION_CATS;
use crate::scanner::{FoundFile, ScanResult, display_path, fmt_age, fmt_size};

const STALE_DAYS: u64 = 180;

pub fn write_full_report(result: &ScanResult, home: &Path, path: &Path) -> io::Result<()> {
    let mut s = String::new();
    s.push_str("diskscout -- full storage cleanup report\n");
    s.push_str(&"=".repeat(60));
    s.push('\n');

    for (section_name, cats) in SECTION_CATS {
        let section_total: u64 = cats.iter().map(|c| result.category_total(*c)).sum();
        if section_total == 0 {
            continue;
        }
        s.push('\n');
        let _ = writeln!(s, "[{}]  {}", section_name, fmt_size(section_total));

        for cat in *cats {
            let dirs = result.dirs_by_category(*cat);
            if dirs.is_empty() {
                continue;
            }
            let cat_total: u64 = dirs.iter().map(|d| d.size).sum();
            let safe_label = if cat.safe_to_delete() {
                "  (safe to delete)"
            } else {
                ""
            };
            s.push('\n');
            let _ = writeln!(
                s,
                "  {}  {}{}",
                cat.label(),
                fmt_size(cat_total),
                safe_label
            );
            for dir in &dirs {
                let _ = writeln!(
                    s,
                    "    {:>10}  {}  ({})",
                    fmt_size(dir.size),
                    display_path(&dir.path, home),
                    fmt_age(dir.last_modified)
                );
            }
        }
    }

    if !result.large_unknown_dirs.is_empty() {
        s.push('\n');
        s.push_str("[UNEXPLORED LARGE DIRECTORIES]  (not matched by any known pattern)\n");
        let stale_threshold = Duration::from_secs(STALE_DAYS * 86_400);
        for d in &result.large_unknown_dirs {
            let stale = SystemTime::now()
                .duration_since(d.last_modified)
                .is_ok_and(|a| a >= stale_threshold);
            let stale_tag = if stale { "  [stale]" } else { "" };
            let _ = writeln!(
                s,
                "\n  {}  {}  (last used: {}){}",
                fmt_size(d.size),
                display_path(&d.path, home),
                fmt_age(d.last_modified),
                stale_tag
            );
            write_children(&mut s, &d.notable_children);
        }
    }

    if !result.large_files.is_empty() {
        s.push('\n');
        s.push_str("[LARGE FILES]  (review before deleting)\n");
        write_files(&mut s, &result.large_files, home);
    }

    if !result.old_files.is_empty() {
        s.push('\n');
        s.push_str("[OLD UNUSED FILES]  (not modified in 6+ months)\n");
        write_files(&mut s, &result.old_files, home);
    }

    if !result.disk_images.is_empty() {
        s.push('\n');
        s.push_str("[DISK IMAGES]  (.dmg / .iso / .pkg) -- forgotten installers\n");
        write_files(&mut s, &result.disk_images, home);
    }

    s.push('\n');
    s.push_str(&"=".repeat(60));
    s.push('\n');
    let _ = writeln!(
        s,
        "TOTAL RECLAIMABLE (dirs + disk images):  {}",
        fmt_size(result.grand_total())
    );
    if result.permission_errors > 0 {
        let _ = writeln!(
            s,
            "note: {} directories skipped due to permission errors",
            result.permission_errors
        );
    }

    let mut f = fs::File::create(path)?;
    f.write_all(s.as_bytes())?;
    Ok(())
}

fn write_files(s: &mut String, files: &[FoundFile], home: &Path) {
    for f in files {
        let _ = writeln!(
            s,
            "    {:>10}  {}  ({})",
            fmt_size(f.size),
            display_path(&f.path, home),
            fmt_age(f.modified)
        );
    }
}

fn write_children(s: &mut String, children: &[(PathBuf, u64)]) {
    for (child, child_size) in children {
        let child_name = child.file_name().and_then(|n| n.to_str()).unwrap_or("?");
        let _ = writeln!(s, "    {:>10}  {}", fmt_size(*child_size), child_name);
    }
}
