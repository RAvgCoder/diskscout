use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::cmp::Reverse;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};
use walkdir::{DirEntry, WalkDir};

use crate::platform;

struct FileLimits {
    large: u64,
    old_after: Duration,
    old_min: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Category {
    // Build artifacts, found by pattern during the walk.
    RustTarget,
    NodeModules,
    PythonBytecode,
    PythonEnv,
    NextJs,
    Nuxt,
    SvelteKit,
    CocoaPods,
    SwiftPmBuild,
    DotNetBuild,
    VisualStudioCache,
    GradleBuild,
    // macOS system directories.
    XcodeDerivedData,
    XcTestDevices,
    IosSimulators,
    SimulatorRuntimes,
    XcodeDeviceSupport,
    XcodeCaches,
    InstrumentsTraces,
    MacOsCaches,
    IphoneBackups,
    // Per-application state outside the walk.
    AppWebCache,
    ContainerCaches,
    ExpensiveCache,
    CloudMirror,
    // Windows system directories.
    WindowsTemp,
    WindowsUpdate,
    WindowsOld,
    RecycleBin,
    CrashDumps,
    WindowsCaches,
    ThumbnailCache,
    InstallerCache,
    BrowserCache,
    // Package and tool caches.
    NpmCache,
    YarnPnpmCache,
    HomebrewCache,
    UvCache,
    PipCache,
    NugetCache,
    CargoRegistry,
    GradleCache,
    MavenCache,
    GoModCache,
    IdeCache,
    // Large payloads that hold real state.
    DockerData,
    AndroidSdk,
    AndroidEmulator,
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
            Category::SwiftPmBuild => "SwiftPM .build/",
            Category::DotNetBuild => ".NET bin/ + obj/",
            Category::VisualStudioCache => "Visual Studio .vs/",
            Category::GradleBuild => "Gradle build/",
            Category::XcodeDerivedData => "Xcode DerivedData",
            Category::XcTestDevices => "XCTest simulator clones",
            Category::IosSimulators => "iOS Simulators",
            Category::SimulatorRuntimes => "Simulator runtimes",
            Category::XcodeDeviceSupport => "Xcode device support",
            Category::XcodeCaches => "Xcode caches",
            Category::InstrumentsTraces => "Instruments traces",
            Category::MacOsCaches => "macOS Caches",
            Category::IphoneBackups => "iPhone Backups",
            Category::AppWebCache => "App web caches",
            Category::ContainerCaches => "App container caches",
            Category::ExpensiveCache => "Expensive caches",
            Category::CloudMirror => "Cloud storage mirror",
            Category::WindowsTemp => "Temp files",
            Category::WindowsUpdate => "Windows Update cache",
            Category::WindowsOld => "Previous Windows install",
            Category::RecycleBin => "Recycle Bin",
            Category::CrashDumps => "Crash dumps & error reports",
            Category::WindowsCaches => "Windows caches",
            Category::ThumbnailCache => "Thumbnail & icon cache",
            Category::InstallerCache => "Installer payload cache",
            Category::BrowserCache => "Browser caches",
            Category::NpmCache => "npm cache",
            Category::YarnPnpmCache => "yarn / pnpm store",
            Category::HomebrewCache => "Homebrew cache",
            Category::UvCache => "uv cache",
            Category::PipCache => "pip cache",
            Category::NugetCache => "NuGet packages",
            Category::CargoRegistry => "Cargo registry",
            Category::GradleCache => "Gradle cache",
            Category::MavenCache => "Maven repository",
            Category::GoModCache => "Go module cache",
            Category::IdeCache => "IDE caches",
            Category::DockerData => "Docker / WSL data",
            Category::AndroidSdk => "Android SDK",
            Category::AndroidEmulator => "Android emulator images",
        }
    }

    /// Regenerable on demand: deleting costs a re-download or a rebuild, never
    /// data. Everything else is reported but flagged for review.
    pub fn safe_to_delete(self) -> bool {
        matches!(
            self,
            Category::RustTarget
                | Category::NodeModules
                | Category::PythonBytecode
                | Category::SwiftPmBuild
                | Category::DotNetBuild
                | Category::VisualStudioCache
                | Category::GradleBuild
                | Category::XcTestDevices
                | Category::MacOsCaches
                | Category::XcodeDeviceSupport
                | Category::XcodeCaches
                | Category::InstrumentsTraces
                | Category::AppWebCache
                | Category::ContainerCaches
                | Category::WindowsTemp
                | Category::WindowsUpdate
                | Category::RecycleBin
                | Category::CrashDumps
                | Category::WindowsCaches
                | Category::ThumbnailCache
                | Category::BrowserCache
                | Category::NpmCache
                | Category::YarnPnpmCache
                | Category::HomebrewCache
                | Category::UvCache
                | Category::PipCache
                | Category::NugetCache
                | Category::CargoRegistry
                | Category::GradleCache
                | Category::MavenCache
                | Category::GoModCache
                | Category::IdeCache
        )
    }

    /// CLI-facing name, derived from the label so the two cannot drift apart.
    /// "Browser caches" becomes "browser-caches".
    pub fn slug(self) -> String {
        let mut out = String::new();
        for ch in self.label().chars() {
            if ch.is_ascii_alphanumeric() {
                out.push(ch.to_ascii_lowercase());
            } else if !out.ends_with('-') {
                out.push('-');
            }
        }
        out.trim_matches('-').to_string()
    }

    /// Live directories the OS and running apps expect to keep existing: empty
    /// them instead of removing the folder itself.
    pub fn delete_contents_only(self) -> bool {
        matches!(
            self,
            Category::WindowsTemp
                | Category::WindowsUpdate
                | Category::RecycleBin
                | Category::CrashDumps
                | Category::WindowsCaches
                | Category::ThumbnailCache
                | Category::BrowserCache
                | Category::AppWebCache
                | Category::ContainerCaches
                | Category::ExpensiveCache
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

// An OS-reserved file that cannot be deleted directly, carried with the
// command that does reclaim it.
#[derive(Debug)]
pub struct ReservedFile {
    pub path: PathBuf,
    pub size: u64,
    pub hint: &'static str,
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub dirs: Vec<FoundDir>,
    pub large_files: Vec<FoundFile>,
    pub old_files: Vec<FoundFile>,
    pub disk_images: Vec<FoundFile>,
    pub large_unknown_dirs: Vec<LargeDir>,
    pub reserved_files: Vec<ReservedFile>,
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

        // Enumerating the fixed targets reads hundreds of directories and shells
        // out to the package managers, so it happens once and every later pass
        // works from the same answer.
        let system: Vec<(PathBuf, Category)> = if self.config.include_system {
            platform::system_targets(&self.config.home)
        } else {
            Vec::new()
        };

        // Those targets are sized directly, so the walker skips them: it avoids
        // both double-counting and a long crawl through cache trees.
        let mut skip = platform::skip_paths(&self.config.home);
        skip.extend(system.iter().map(|(path, _)| path.clone()));

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

        pending.extend(system);
        pending.extend(platform::build_cache_targets(&self.config.home));

        let pending = keep_outermost(pending);

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

        // An empty target is either genuinely empty or unreadable without
        // elevation; either way there is nothing to report.
        dirs.retain(|d| d.size > 0);
        dirs.sort_by_key(|d| Reverse(d.size));

        large_files.sort_by_key(|f| Reverse(f.size));
        large_files.dedup_by(|a, b| a.path == b.path);
        large_files.truncate(30);

        old_files.sort_by_key(|f| Reverse(f.size));
        old_files.dedup_by(|a, b| a.path == b.path);
        old_files.truncate(30);

        disk_images.sort_by_key(|f| Reverse(f.size));

        let large_unknown_dirs = self.scan_unknown_large_dirs(&dirs, &skip);

        ScanResult {
            dirs,
            large_files,
            old_files,
            disk_images,
            large_unknown_dirs,
            reserved_files: reserved_files(),
            permission_errors: err_count.load(Ordering::Relaxed),
        }
    }

    // Second pass: walk to depth 4 looking for large dirs not covered by any
    // known category. Surfaces things like VM images, model weights, forgotten
    // project archives -- anything the pattern matcher wouldn't catch.
    fn scan_unknown_large_dirs(&self, categorized: &[FoundDir], skip: &[PathBuf]) -> Vec<LargeDir> {
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
        if platform::occupies_no_local_space(&meta) {
            return;
        }
        let size = platform::size_on_disk(entry.path(), &meta);
        let modified = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        let path = entry.path().to_owned();

        if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && platform::is_disk_image(&ext.to_lowercase())
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
}

fn reserved_files() -> Vec<ReservedFile> {
    platform::reserved_system_files()
        .into_iter()
        .filter_map(|(path, hint)| {
            let size = std::fs::metadata(&path).ok()?.len();
            Some(ReservedFile { path, size, hint })
        })
        .collect()
}

fn classify_dir(entry: &DirEntry) -> Option<Category> {
    let name = entry.file_name().to_str()?;
    let parent = entry.path().parent()?;

    if is_inside_app_bundle(entry.path()) {
        return None;
    }

    match name {
        "target" => {
            if parent.join("Cargo.toml").exists() || parent.join("Cargo.lock").exists() {
                Some(Category::RustTarget)
            } else {
                None
            }
        }
        "node_modules" => {
            if is_packaged_app(entry.path()) {
                None
            } else {
                Some(Category::NodeModules)
            }
        }
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
        // "bin" and "obj" are common enough names that they only count next to
        // a project file that explains them.
        "bin" | "obj" => {
            if has_sibling_ext(parent, &["csproj", "vbproj", "fsproj", "vcxproj", "sln"]) {
                Some(Category::DotNetBuild)
            } else {
                None
            }
        }
        ".vs" => Some(Category::VisualStudioCache),
        // Gradle writes all three next to the build script: the output tree, a
        // project-local daemon cache, and the NDK's native build.
        "build" | ".gradle" | ".cxx" => {
            if parent.join("build.gradle").exists()
                || parent.join("build.gradle.kts").exists()
                || parent.join("settings.gradle").exists()
                || parent.join("settings.gradle.kts").exists()
            {
                Some(Category::GradleBuild)
            } else {
                None
            }
        }
        ".build" => {
            if parent.join("Package.swift").exists() {
                Some(Category::SwiftPmBuild)
            } else {
                None
            }
        }
        // Xcode's default is the shared one under ~/Library, but a project can
        // redirect it next to the sources.
        "DerivedData" => Some(Category::XcodeDerivedData),
        "packages" => {
            if has_sibling_ext(parent, &["sln"]) {
                Some(Category::NugetCache)
            } else {
                None
            }
        }
        _ => None,
    }
}

// An Electron app ships its dependencies inside its own install directory, so
// the tree looks exactly like a project's `node_modules` -- but deleting it
// breaks the installed application rather than freeing a rebuildable cache.
/// Everything inside a macOS bundle ships with the app and is covered by its
/// code signature, so removing any of it breaks the signature rather than
/// freeing a rebuildable cache -- an IDE's bundled plugin dependencies and the
/// bytecode caches of the Python it embeds both live in here.
fn is_inside_app_bundle(path: &Path) -> bool {
    path.iter()
        .any(|c| c.to_str().is_some_and(|c| c.ends_with(".app")))
}

fn is_packaged_app(path: &Path) -> bool {
    let mut components = path.iter().rev().skip(1); // skip "node_modules"
    let parent = components.next().and_then(|c| c.to_str());
    if matches!(parent, Some("app.asar.unpacked")) {
        return true;
    }
    // .../resources/app/node_modules and .../resources/node_modules
    let grandparent = components.next().and_then(|c| c.to_str());
    matches!(parent, Some("resources"))
        || (matches!(parent, Some("app")) && matches!(grandparent, Some("resources")))
}

// Whether `dir` directly contains a file with any of these extensions.
fn has_sibling_ext(dir: &Path, exts: &[&str]) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries.flatten().any(|e| {
        e.path()
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| exts.iter().any(|want| x.eq_ignore_ascii_case(want)))
    })
}

/// Two targets can name the same directory (%TEMP% under a relocated profile),
/// and one can sit inside another (a sandboxed app's cache inside its own
/// container). Sizing either twice inflates the total, so only the outermost of
/// an overlap survives. Path order puts an ancestor ahead of everything under it.
fn keep_outermost(mut targets: Vec<(PathBuf, Category)>) -> Vec<(PathBuf, Category)> {
    targets.sort_by(|a, b| a.0.cmp(&b.0));

    let mut kept: Vec<(PathBuf, Category)> = Vec::with_capacity(targets.len());
    for (path, category) in targets {
        if !kept.iter().any(|(outer, _)| path.starts_with(outer)) {
            kept.push((path, category));
        }
    }
    kept
}

// Computes total size and most-recent modification time in a single walk.
// Hard links are counted once per walk, so a blob linked from two targets is
// still counted twice; nothing observed links across targets.
pub fn dir_stats(path: &Path) -> (u64, SystemTime) {
    let mut size = 0u64;
    let mut newest = SystemTime::UNIX_EPOCH;
    let mut linked = HashSet::new();
    for entry in WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(std::result::Result::ok)
    {
        if entry.file_type().is_file()
            && let Ok(meta) = entry.metadata()
            && !platform::occupies_no_local_space(&meta)
        {
            if platform::hard_link_id(&meta).is_some_and(|id| !linked.insert(id)) {
                continue;
            }
            size += platform::size_on_disk(entry.path(), &meta);
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
        format!("{bytes} B")
    }
}

pub fn fmt_age(modified: SystemTime) -> String {
    match SystemTime::now().duration_since(modified) {
        Ok(age) => {
            let days = age.as_secs() / 86_400;
            if days < 7 {
                format!("{days} days")
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
        Ok(rel) => format!("~{}{}", std::path::MAIN_SEPARATOR, rel.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    fn classify(root: &Path, name: &str) -> Option<Category> {
        WalkDir::new(root)
            .into_iter()
            .filter_map(std::result::Result::ok)
            .find(|e| e.file_type().is_dir() && e.file_name() == name)
            .and_then(|e| classify_dir(&e))
    }

    #[test]
    fn fmt_size_picks_the_largest_fitting_unit() {
        assert_eq!(fmt_size(0), "0 B");
        assert_eq!(fmt_size(1_023), "1023 B");
        assert_eq!(fmt_size(1_024), "1 KB");
        assert_eq!(fmt_size(1_048_576), "1 MB");
        assert_eq!(fmt_size(1_073_741_824), "1.0 GB");
        assert_eq!(fmt_size(3_221_225_472), "3.0 GB");
    }

    #[test]
    fn fmt_age_scales_from_days_to_years() {
        let ago = |secs| SystemTime::now() - Duration::from_secs(secs);
        assert_eq!(fmt_age(ago(0)), "0 days");
        assert_eq!(fmt_age(ago(6 * 86_400)), "6 days");
        assert_eq!(fmt_age(ago(14 * 86_400)), "2 weeks");
        assert_eq!(fmt_age(ago(60 * 86_400)), "2 months");
        assert_eq!(fmt_age(ago(730 * 86_400)), "2.0 years");
    }

    #[test]
    fn fmt_age_reports_future_timestamps_as_unknown() {
        assert_eq!(
            fmt_age(SystemTime::now() + Duration::from_secs(86_400)),
            "unknown age"
        );
    }

    #[test]
    fn display_path_abbreviates_home() {
        let sep = std::path::MAIN_SEPARATOR;
        let home = Path::new("/Users/someone");
        assert_eq!(display_path(home, home), "~");
        assert_eq!(
            display_path(&home.join("Developer").join("x"), home),
            format!("~{sep}Developer{sep}x")
        );
        assert_eq!(
            display_path(Path::new("/opt/homebrew"), home),
            "/opt/homebrew"
        );
    }

    #[test]
    fn target_counts_only_beside_a_cargo_manifest() {
        let td = tempdir().unwrap();
        let crate_dir = td.path().join("mycrate");
        fs::create_dir_all(crate_dir.join("target")).unwrap();
        fs::write(crate_dir.join("Cargo.toml"), "[package]").unwrap();
        assert_eq!(classify(td.path(), "target"), Some(Category::RustTarget));

        let td2 = tempdir().unwrap();
        fs::create_dir_all(td2.path().join("somewhere/target")).unwrap();
        assert_eq!(classify(td2.path(), "target"), None);
    }

    #[test]
    fn node_modules_and_pycache_need_no_marker_file() {
        let td = tempdir().unwrap();
        fs::create_dir_all(td.path().join("app/node_modules")).unwrap();
        fs::create_dir_all(td.path().join("app/__pycache__")).unwrap();
        assert_eq!(
            classify(td.path(), "node_modules"),
            Some(Category::NodeModules)
        );
        assert_eq!(
            classify(td.path(), "__pycache__"),
            Some(Category::PythonBytecode)
        );
    }

    #[test]
    fn venv_counts_only_beside_a_python_project_marker() {
        let td = tempdir().unwrap();
        let proj = td.path().join("proj");
        fs::create_dir_all(proj.join(".venv")).unwrap();
        fs::write(proj.join("pyproject.toml"), "").unwrap();
        assert_eq!(classify(td.path(), ".venv"), Some(Category::PythonEnv));

        let td2 = tempdir().unwrap();
        fs::create_dir_all(td2.path().join("notaproject/.venv")).unwrap();
        assert_eq!(classify(td2.path(), ".venv"), None);
    }

    #[test]
    fn pods_counts_only_beside_a_podfile() {
        let td = tempdir().unwrap();
        let app = td.path().join("App");
        fs::create_dir_all(app.join("Pods")).unwrap();
        fs::write(app.join("Podfile"), "").unwrap();
        assert_eq!(classify(td.path(), "Pods"), Some(Category::CocoaPods));

        let td2 = tempdir().unwrap();
        fs::create_dir_all(td2.path().join("x/Pods")).unwrap();
        assert_eq!(classify(td2.path(), "Pods"), None);
    }

    #[test]
    fn bin_and_obj_count_only_beside_a_project_file() {
        let td = tempdir().unwrap();
        let proj = td.path().join("App");
        fs::create_dir_all(proj.join("bin")).unwrap();
        fs::create_dir_all(proj.join("obj")).unwrap();
        fs::write(proj.join("App.csproj"), "").unwrap();
        assert_eq!(classify(td.path(), "bin"), Some(Category::DotNetBuild));
        assert_eq!(classify(td.path(), "obj"), Some(Category::DotNetBuild));

        // A stray bin/ is just a directory named bin.
        let td2 = tempdir().unwrap();
        fs::create_dir_all(td2.path().join("tools/bin")).unwrap();
        assert_eq!(classify(td2.path(), "bin"), None);
    }

    #[test]
    fn gradle_build_counts_only_beside_a_gradle_script() {
        let td = tempdir().unwrap();
        let proj = td.path().join("app");
        fs::create_dir_all(proj.join("build")).unwrap();
        fs::write(proj.join("build.gradle.kts"), "").unwrap();
        assert_eq!(classify(td.path(), "build"), Some(Category::GradleBuild));

        let td2 = tempdir().unwrap();
        fs::create_dir_all(td2.path().join("cmake/build")).unwrap();
        assert_eq!(classify(td2.path(), "build"), None);
    }

    #[test]
    fn gradle_daemon_and_ndk_dirs_count_beside_a_settings_script() {
        let td = tempdir().unwrap();
        let proj = td.path().join("app");
        fs::create_dir_all(proj.join(".gradle")).unwrap();
        fs::create_dir_all(proj.join(".cxx")).unwrap();
        fs::write(proj.join("settings.gradle"), "").unwrap();

        assert_eq!(classify(td.path(), ".gradle"), Some(Category::GradleBuild));
        assert_eq!(classify(td.path(), ".cxx"), Some(Category::GradleBuild));

        // ~/.gradle is the user-wide cache, a different category entirely.
        let bare = tempdir().unwrap();
        fs::create_dir_all(bare.path().join("home/.gradle")).unwrap();
        assert_eq!(classify(bare.path(), ".gradle"), None);
    }

    #[test]
    fn swiftpm_build_counts_only_beside_a_package_manifest() {
        let td = tempdir().unwrap();
        let proj = td.path().join("Lib");
        fs::create_dir_all(proj.join(".build")).unwrap();
        fs::write(proj.join("Package.swift"), "").unwrap();
        assert_eq!(classify(td.path(), ".build"), Some(Category::SwiftPmBuild));

        let td2 = tempdir().unwrap();
        fs::create_dir_all(td2.path().join("other/.build")).unwrap();
        assert_eq!(classify(td2.path(), ".build"), None);
    }

    #[test]
    fn a_project_local_derived_data_is_found_by_the_walk() {
        let td = tempdir().unwrap();
        fs::create_dir_all(td.path().join("App/DerivedData")).unwrap();
        assert_eq!(
            classify(td.path(), "DerivedData"),
            Some(Category::XcodeDerivedData)
        );
    }

    #[test]
    fn a_target_inside_another_target_is_dropped() {
        let container = PathBuf::from("/Users/x/Library/Containers/com.docker.docker/Data");
        let inner = container.join("Library/Caches");
        let sibling = PathBuf::from("/Users/x/Library/Containers/com.other.app/Data");

        let kept = keep_outermost(vec![
            (inner.clone(), Category::ContainerCaches),
            (container.clone(), Category::DockerData),
            (sibling.clone(), Category::ContainerCaches),
            (container.clone(), Category::DockerData),
        ]);

        let paths: Vec<&PathBuf> = kept.iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec![&container, &sibling], "{kept:?}");
    }

    // A name that merely starts with another's text is not inside it.
    #[test]
    fn keep_outermost_compares_whole_path_components() {
        let dir = PathBuf::from("/a/b");
        let lookalike = PathBuf::from("/a/b-backup");

        let kept = keep_outermost(vec![
            (lookalike.clone(), Category::MacOsCaches),
            (dir.clone(), Category::MacOsCaches),
        ]);

        assert_eq!(kept.len(), 2, "{kept:?}");
    }

    #[cfg(not(windows))]
    #[test]
    fn a_hard_linked_file_is_counted_once() {
        let td = tempdir().unwrap();
        let original = td.path().join("blob");
        fs::write(&original, vec![0u8; 8_192]).unwrap();
        fs::hard_link(&original, td.path().join("linked")).unwrap();

        let (size, _) = dir_stats(td.path());
        assert_eq!(size, 8_192, "the second link is the same allocation");
    }

    #[test]
    fn vs_cache_needs_no_marker_file() {
        let td = tempdir().unwrap();
        fs::create_dir_all(td.path().join("Sln/.vs")).unwrap();
        assert_eq!(
            classify(td.path(), ".vs"),
            Some(Category::VisualStudioCache)
        );
    }

    #[test]
    fn only_regenerable_categories_are_marked_safe() {
        assert!(Category::RustTarget.safe_to_delete());
        assert!(Category::NodeModules.safe_to_delete());
        assert!(Category::UvCache.safe_to_delete());
        assert!(Category::WindowsTemp.safe_to_delete());
        assert!(Category::RecycleBin.safe_to_delete());
        assert!(!Category::IphoneBackups.safe_to_delete());
        assert!(!Category::IosSimulators.safe_to_delete());
        assert!(!Category::PythonEnv.safe_to_delete());
        // Deleting these breaks repair/uninstall or throws away real state.
        assert!(!Category::WindowsOld.safe_to_delete());
        assert!(!Category::InstallerCache.safe_to_delete());
        assert!(!Category::DockerData.safe_to_delete());
        // Reported so the space is visible, never auto-deleted: one is real
        // data behind a placeholder, the other needs root and a re-download.
        assert!(!Category::CloudMirror.safe_to_delete());
        assert!(!Category::SimulatorRuntimes.safe_to_delete());
        assert!(!Category::AndroidEmulator.safe_to_delete());
    }

    // Removing any of it breaks the bundle's code signature, and an IDE ships
    // both its plugins' dependencies and its embedded Python's bytecode.
    #[test]
    fn nothing_inside_an_app_bundle_is_reclaimable() {
        let td = tempdir().unwrap();
        let plugin = td
            .path()
            .join("Applications/CLion.app/Contents/plugins/vuejs/node_modules");
        let bytecode = td
            .path()
            .join("Applications/CLion.app/Contents/python/lib/__pycache__");
        fs::create_dir_all(&plugin).unwrap();
        fs::create_dir_all(&bytecode).unwrap();
        fs::write(plugin.parent().unwrap().join("package.json"), "{}").unwrap();

        assert_eq!(classify(td.path(), "node_modules"), None);
        assert_eq!(classify(td.path(), "__pycache__"), None);
    }

    #[test]
    fn an_electron_apps_bundled_deps_are_not_reclaimable() {
        let td = tempdir().unwrap();
        // The layout that made diskscout try to gut Claude Desktop.
        let packaged = td
            .path()
            .join("SomeApp/app-1.2.3/resources/app.asar.unpacked");
        fs::create_dir_all(packaged.join("node_modules")).unwrap();
        assert_eq!(classify(td.path(), "node_modules"), None);

        let td2 = tempdir().unwrap();
        fs::create_dir_all(td2.path().join("OtherApp/resources/app/node_modules")).unwrap();
        assert_eq!(classify(td2.path(), "node_modules"), None);

        // A real project is still caught.
        let td3 = tempdir().unwrap();
        fs::create_dir_all(td3.path().join("myproject/node_modules")).unwrap();
        assert_eq!(
            classify(td3.path(), "node_modules"),
            Some(Category::NodeModules)
        );
    }

    #[test]
    fn slugs_are_cli_safe_and_unique() {
        assert_eq!(Category::BrowserCache.slug(), "browser-caches");
        assert_eq!(Category::RustTarget.slug(), "rust-target");
        assert_eq!(Category::DotNetBuild.slug(), "net-bin-obj");
        assert_eq!(Category::YarnPnpmCache.slug(), "yarn-pnpm-store");

        let all: Vec<String> = crate::report::all_categories()
            .map(Category::slug)
            .collect();
        let mut unique = all.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(all.len(), unique.len(), "two categories share a slug");
        assert!(
            all.iter().all(|s| !s.is_empty()
                && s.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')),
            "a slug would need shell quoting"
        );
    }

    #[test]
    fn live_os_directories_are_emptied_not_removed() {
        assert!(Category::WindowsTemp.delete_contents_only());
        assert!(Category::RecycleBin.delete_contents_only());
        assert!(Category::BrowserCache.delete_contents_only());
        // Project-local artifacts go away wholesale.
        assert!(!Category::RustTarget.delete_contents_only());
        assert!(!Category::NodeModules.delete_contents_only());
    }

    #[test]
    fn dir_stats_sums_nested_files() {
        let td = tempdir().unwrap();
        fs::create_dir_all(td.path().join("a/b")).unwrap();
        fs::write(td.path().join("a/one.bin"), vec![0u8; 100]).unwrap();
        fs::write(td.path().join("a/b/two.bin"), vec![0u8; 250]).unwrap();
        let (size, newest) = dir_stats(td.path());
        assert_eq!(size, 350);
        assert!(newest > SystemTime::UNIX_EPOCH);
    }

    #[test]
    fn dir_stats_on_an_empty_dir_is_zero() {
        let td = tempdir().unwrap();
        let (size, newest) = dir_stats(td.path());
        assert_eq!(size, 0);
        assert_eq!(newest, SystemTime::UNIX_EPOCH);
    }

    fn found(path: &str, category: Category, size: u64) -> FoundDir {
        FoundDir {
            path: PathBuf::from(path),
            category,
            size,
            last_modified: SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn dirs_by_category_filters_and_sorts_largest_first() {
        let result = ScanResult {
            dirs: vec![
                found("/a", Category::RustTarget, 10),
                found("/b", Category::NodeModules, 999),
                found("/c", Category::RustTarget, 500),
            ],
            ..ScanResult::default()
        };
        let targets = result.dirs_by_category(Category::RustTarget);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].size, 500);
        assert_eq!(targets[1].size, 10);
        assert_eq!(result.category_total(Category::RustTarget), 510);
    }

    #[test]
    fn grand_total_counts_dirs_and_disk_images() {
        let result = ScanResult {
            dirs: vec![found("/a", Category::RustTarget, 100)],
            disk_images: vec![FoundFile {
                path: PathBuf::from("/x.dmg"),
                size: 25,
                modified: SystemTime::UNIX_EPOCH,
            }],
            ..ScanResult::default()
        };
        assert_eq!(result.grand_total(), 125);
    }
}
