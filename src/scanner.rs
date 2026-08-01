use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use walkdir::{DirEntry, WalkDir};

struct FileLimits {
    large: u64,
    old_after: Duration,
    old_min: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
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
}

impl Category {
    pub fn label(self) -> &'static str {
        match self {
            Category::RustTarget => "Rust target/",
            Category::NodeModules => "node_modules/",
            Category::PythonBytecode => "Python __pycache__",
            Category::PythonEnv => "Python virtualenv",
            Category::NextJs => "Next.js .next/",
            Category::Nuxt => "Nuxt .nuxt/",
            Category::SvelteKit => "SvelteKit .svelte-kit/",
            Category::CocoaPods => "CocoaPods Pods/",
            Category::XcodeDerivedData => "Xcode DerivedData",
            Category::IosSimulators => "iOS Simulators",
            Category::MacOsCaches => "macOS Caches",
            Category::IphoneBackups => "iPhone Backups",
            Category::NpmCache => "npm cache",
            Category::HomebrewCache => "Homebrew cache",
            Category::UvCache => "uv cache",
        }
    }

    pub fn safe_to_delete(self) -> bool {
        matches!(
            self,
            Category::RustTarget
                | Category::NodeModules
                | Category::PythonBytecode
                | Category::NpmCache
                | Category::HomebrewCache
                | Category::UvCache
        )
    }
}

pub struct ScanConfig {
    pub root: PathBuf,
    pub home: PathBuf,
    pub old_after_days: u64,
    pub large_over_mb: u64,
    pub include_system: bool,
}

#[derive(Debug)]
pub struct FoundDir {
    pub path: PathBuf,
    pub category: Category,
    pub size: u64,
    pub last_modified: SystemTime,
}

#[derive(Debug)]
pub struct FoundFile {
    pub path: PathBuf,
    pub size: u64,
    pub modified: SystemTime,
}

// A large directory not matched by any known category pattern.
#[derive(Debug)]
pub struct LargeDir {
    pub path: PathBuf,
    pub size: u64,
    pub last_modified: SystemTime,
    // Top immediate subdirs by size, so the report can show what's inside.
    pub notable_children: Vec<(PathBuf, u64)>,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub dirs: Vec<FoundDir>,
    pub large_files: Vec<FoundFile>,
    pub old_files: Vec<FoundFile>,
    pub disk_images: Vec<FoundFile>,
    pub large_unknown_dirs: Vec<LargeDir>,
    pub permission_errors: usize,
}

impl ScanResult {
    pub fn dirs_by_category(&self, cat: Category) -> Vec<&FoundDir> {
        let mut v: Vec<&FoundDir> = self.dirs.iter().filter(|d| d.category == cat).collect();
        v.sort_by_key(|d| Reverse(d.size));
        v
    }

    pub fn category_total(&self, cat: Category) -> u64 {
        self.dirs
            .iter()
            .filter(|d| d.category == cat)
            .map(|d| d.size)
            .sum()
    }

    pub fn grand_total(&self) -> u64 {
        let dir_total: u64 = self.dirs.iter().map(|d| d.size).sum();
        let img_total: u64 = self.disk_images.iter().map(|f| f.size).sum();
        dir_total + img_total
    }
}

pub struct Scanner {
    config: ScanConfig,
}

impl Scanner {
    pub fn new(config: ScanConfig) -> Self {
        Self { config }
    }

    fn skip_paths(&self) -> Vec<PathBuf> {
        vec![
            PathBuf::from("/System"),
            PathBuf::from("/private"),
            PathBuf::from("/dev"),
            PathBuf::from("/Volumes"),
            PathBuf::from("/proc"),
            self.config.home.join("Library"),
            self.config.home.join(".Trash"),
        ]
    }

    pub fn scan(&self) -> ScanResult {
        let err_count = AtomicUsize::new(0);
        let mut pending: Vec<(PathBuf, Category)> = Vec::new();
        let mut large_files: Vec<FoundFile> = Vec::new();
        let mut old_files: Vec<FoundFile> = Vec::new();
        let mut disk_images: Vec<FoundFile> = Vec::new();

        let limits = FileLimits {
            large: self.config.large_over_mb * 1_048_576,
            old_after: Duration::from_secs(self.config.old_after_days * 86_400),
            old_min: 10 * 1_048_576,
        };

        let skip = self.skip_paths();

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        spinner.set_message("Scanning filesystem...");
        spinner.enable_steady_tick(Duration::from_millis(80));

        let mut walker = WalkDir::new(&self.config.root)
            .follow_links(false)
            .into_iter();

        loop {
            let entry = match walker.next() {
                None => break,
                Some(Err(_)) => {
                    err_count.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                Some(Ok(e)) => e,
            };

            let path = entry.path();

            if skip.iter().any(|s| path.starts_with(s)) {
                walker.skip_current_dir();
                continue;
            }

            if entry.file_type().is_dir()
                && entry.file_name().to_str() == Some(".git")
                && entry.depth() > 0
            {
                walker.skip_current_dir();
                continue;
            }

            if entry.file_type().is_dir()
                && entry.depth() > 0
                && let Some(cat) = classify_dir(&entry)
            {
                pending.push((path.to_owned(), cat));
                walker.skip_current_dir();
                continue;
            }

            if entry.file_type().is_file() {
                self.check_file(
                    &entry,
                    &limits,
                    &mut large_files,
                    &mut old_files,
                    &mut disk_images,
                );
            }
        }

        spinner.finish_and_clear();

        if self.config.include_system {
            pending.extend(self.fixed_macos_paths());
        }

        // Check both the persistent build cache location and the legacy /tmp path.
        for cargo_target in [
            self.config.home.join(".build_caches/cargo"),
            PathBuf::from("/tmp/cargo-target"),
        ] {
            if cargo_target.exists() {
                pending.push((cargo_target, Category::RustTarget));
            }
        }

        eprintln!("Found {} directories -- computing sizes...", pending.len());

        let pb = ProgressBar::new(pending.len() as u64);
        pb.set_style(
            ProgressStyle::with_template("  [{bar:40.cyan/blue}] {pos}/{len} ({eta} remaining)")
                .unwrap()
                .progress_chars("=> "),
        );

        let mut dirs: Vec<FoundDir> = pending
            .into_par_iter()
            .map(|(path, category)| {
                let (size, last_modified) = dir_stats(&path);
                pb.inc(1);
                FoundDir {
                    path,
                    category,
                    size,
                    last_modified,
                }
            })
            .collect();

        pb.finish_and_clear();

        dirs.sort_by_key(|d| Reverse(d.size));

        large_files.sort_by_key(|f| Reverse(f.size));
        large_files.dedup_by(|a, b| a.path == b.path);
        large_files.truncate(30);

        old_files.sort_by_key(|f| Reverse(f.size));
        old_files.dedup_by(|a, b| a.path == b.path);
        old_files.truncate(30);

        disk_images.sort_by_key(|f| Reverse(f.size));

        let large_unknown_dirs = self.scan_unknown_large_dirs(&dirs);

        ScanResult {
            dirs,
            large_files,
            old_files,
            disk_images,
            large_unknown_dirs,
            permission_errors: err_count.load(Ordering::Relaxed),
        }
    }

    // Second pass: walk to depth 4 looking for large dirs not covered by any
    // known category. Surfaces things like VM images, model weights, forgotten
    // project archives -- anything the pattern matcher wouldn't catch.
    fn scan_unknown_large_dirs(&self, categorized: &[FoundDir]) -> Vec<LargeDir> {
        let skip = self.skip_paths();

        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        spinner.set_message("Scanning for unexplored large directories...");
        spinner.enable_steady_tick(Duration::from_millis(80));

        let mut candidates: Vec<PathBuf> = Vec::new();
        let mut walker = WalkDir::new(&self.config.root)
            .follow_links(false)
            .max_depth(4)
            .into_iter();

        loop {
            let entry = match walker.next() {
                None => break,
                Some(Err(_)) => continue,
                Some(Ok(e)) => e,
            };

            let path = entry.path();

            if skip.iter().any(|s| path.starts_with(s)) {
                walker.skip_current_dir();
                continue;
            }

            if !entry.file_type().is_dir() || entry.depth() == 0 {
                continue;
            }

            if entry.file_name().to_str() == Some(".git") {
                walker.skip_current_dir();
                continue;
            }

            // Already accounted for in the main categorized report.
            if categorized.iter().any(|c| path.starts_with(&c.path)) {
                walker.skip_current_dir();
                continue;
            }

            candidates.push(path.to_owned());
        }

        spinner.finish_and_clear();

        eprintln!(
            "Found {} unexplored directories -- computing sizes...",
            candidates.len()
        );

        let pb = ProgressBar::new(candidates.len() as u64);
        pb.set_style(
            ProgressStyle::with_template("  [{bar:40.cyan/blue}] {pos}/{len} ({eta} remaining)")
                .unwrap()
                .progress_chars("=> "),
        );

        let mut all_stats: Vec<(PathBuf, u64, SystemTime)> = candidates
            .into_par_iter()
            .map(|path| {
                let (size, last_modified) = dir_stats(&path);
                pb.inc(1);
                (path, size, last_modified)
            })
            .collect();

        pb.finish_and_clear();

        all_stats.retain(|(_, size, _)| *size >= 50 * 1_048_576);
        all_stats.sort_by_key(|s| Reverse(s.1));

        // Greedy dedup: keep a dir only if no ancestor is already in the result.
        // Attach the top immediate children so the report can show what's inside.
        let mut result: Vec<LargeDir> = Vec::new();
        for (path, size, last_modified) in &all_stats {
            if result.iter().any(|r| path.starts_with(&r.path)) {
                continue;
            }

            let notable_children: Vec<(PathBuf, u64)> = all_stats
                .iter()
                .filter(|(c, _, _)| c.parent() == Some(path.as_path()))
                .take(4)
                .map(|(c, s, _)| (c.clone(), *s))
                .collect();

            result.push(LargeDir {
                path: path.clone(),
                size: *size,
                last_modified: *last_modified,
                notable_children,
            });

            if result.len() >= 25 {
                break;
            }
        }

        result
    }

    fn check_file(
        &self,
        entry: &DirEntry,
        limits: &FileLimits,
        large: &mut Vec<FoundFile>,
        old: &mut Vec<FoundFile>,
        images: &mut Vec<FoundFile>,
    ) {
        let Ok(meta) = entry.metadata() else {
            return;
        };
        let size = meta.len();
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let path = entry.path().to_owned();

        if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && matches!(ext.to_lowercase().as_str(), "dmg" | "iso" | "pkg")
            && size > 1_048_576
        {
            images.push(FoundFile {
                path: path.clone(),
                size,
                modified,
            });
        }

        if size >= limits.large {
            large.push(FoundFile {
                path: path.clone(),
                size,
                modified,
            });
        }

        if size >= limits.old_min
            && let Ok(age) = SystemTime::now().duration_since(modified)
            && age >= limits.old_after
        {
            old.push(FoundFile {
                path,
                size,
                modified,
            });
        }
    }

    fn fixed_macos_paths(&self) -> Vec<(PathBuf, Category)> {
        let h = &self.config.home;
        let mut out = Vec::new();

        let candidates = [
            // Resolve symlink so ~/.build_caches/xcode and the DerivedData symlink
            // don't both end up in the list and get double-counted.
            (
                std::fs::canonicalize(h.join("Library/Developer/Xcode/DerivedData"))
                    .unwrap_or_else(|_| h.join("Library/Developer/Xcode/DerivedData")),
                Category::XcodeDerivedData,
            ),
            (
                h.join("Library/Developer/CoreSimulator/Devices"),
                Category::IosSimulators,
            ),
            (h.join("Library/Caches"), Category::MacOsCaches),
            (
                h.join("Library/Application Support/MobileSync/Backup"),
                Category::IphoneBackups,
            ),
            (h.join(".npm/_cacache"), Category::NpmCache),
            (h.join(".cache/uv"), Category::UvCache),
            (h.join("Library/Caches/uv"), Category::UvCache),
            (h.join(".cache/pip"), Category::UvCache),
        ];

        for (path, cat) in candidates {
            if path.exists() {
                out.push((path, cat));
            }
        }

        if let Ok(out_bytes) = std::process::Command::new("brew").arg("--cache").output()
            && out_bytes.status.success()
        {
            let brew_path = PathBuf::from(
                String::from_utf8_lossy(&out_bytes.stdout)
                    .trim()
                    .to_string(),
            );
            if brew_path.exists() {
                out.push((brew_path, Category::HomebrewCache));
            }
        }

        out
    }
}

fn classify_dir(entry: &DirEntry) -> Option<Category> {
    let name = entry.file_name().to_str()?;
    let parent = entry.path().parent()?;

    match name {
        "target" => {
            if parent.join("Cargo.toml").exists() || parent.join("Cargo.lock").exists() {
                Some(Category::RustTarget)
            } else {
                None
            }
        }
        "node_modules" => Some(Category::NodeModules),
        "__pycache__" => Some(Category::PythonBytecode),
        ".venv" | "venv" | "env" => {
            if parent.join("pyproject.toml").exists()
                || parent.join("requirements.txt").exists()
                || parent.join("setup.py").exists()
                || parent.join("setup.cfg").exists()
            {
                Some(Category::PythonEnv)
            } else {
                None
            }
        }
        ".next" => Some(Category::NextJs),
        ".nuxt" => Some(Category::Nuxt),
        ".svelte-kit" => Some(Category::SvelteKit),
        "Pods" => {
            if parent.join("Podfile").exists() {
                Some(Category::CocoaPods)
            } else {
                None
            }
        }
        _ => None,
    }
}

// Computes total size and most-recent modification time in a single walk.
pub fn dir_stats(path: &Path) -> (u64, SystemTime) {
    let mut size = 0u64;
    let mut newest = SystemTime::UNIX_EPOCH;
    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_type().is_file()
            && let Ok(meta) = entry.metadata()
        {
            size += meta.len();
            if let Ok(modified) = meta.modified()
                && modified > newest
            {
                newest = modified;
            }
        }
    }
    (size, newest)
}

pub fn fmt_size(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    } else if bytes >= 1_024 {
        format!("{:.0} KB", bytes as f64 / 1_024.0)
    } else {
        format!("{} B", bytes)
    }
}

pub fn fmt_age(modified: SystemTime) -> String {
    match SystemTime::now().duration_since(modified) {
        Ok(age) => {
            let days = age.as_secs() / 86_400;
            if days < 7 {
                format!("{} days", days)
            } else if days < 30 {
                format!("{} weeks", days / 7)
            } else if days < 365 {
                format!("{} months", days / 30)
            } else {
                format!("{:.1} years", days as f64 / 365.0)
            }
        }
        Err(_) => "unknown age".to_string(),
    }
}

pub fn display_path(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rel) if rel.as_os_str().is_empty() => "~".to_string(),
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => path.display().to_string(),
    }
}
