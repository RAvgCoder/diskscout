//! Everything that differs between macOS/Linux and Windows: where home and temp
//! live, which roots the walker must not descend into, and the fixed OS
//! directories that never turn up as a pattern match during the walk.
//!
//! Keeping it all here lets `scanner`, `report` and `cleaner` stay free of
//! `cfg` noise -- they just ask for a list and size whatever comes back.

use std::path::{Path, PathBuf};

use crate::scanner::Category;

/// Name of the environment variable that points at the user's home, used only
/// to write a sensible error message when it is missing.
#[cfg(windows)]
pub const HOME_VAR: &str = "%USERPROFILE%";
#[cfg(not(windows))]
pub const HOME_VAR: &str = "$HOME";

/// The user's home directory. Windows has no `$HOME`; `%USERPROFILE%` is the
/// equivalent, with `%HOMEDRIVE%%HOMEPATH%` as the domain-joined fallback.
pub fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        if let Some(profile) = env_path("USERPROFILE") {
            return Some(profile);
        }
        let mut joined = std::env::var_os("HOMEDRIVE")?;
        joined.push(std::env::var_os("HOMEPATH")?);
        Some(PathBuf::from(joined))
    }
    #[cfg(not(windows))]
    {
        env_path("HOME")
    }
}

/// Where the full untruncated report is written.
pub fn report_path() -> PathBuf {
    #[cfg(windows)]
    {
        std::env::temp_dir().join("diskscout-report.txt")
    }
    #[cfg(not(windows))]
    {
        PathBuf::from("/tmp/diskscout-report.txt")
    }
}

/// Extra advice printed under the report footer, for reclaimable space that
/// this tool can measure but should not delete itself.
pub fn extra_hints() -> &'static [&'static str] {
    #[cfg(windows)]
    {
        &[
            "Run from an elevated terminal to measure C:\\Windows and other system dirs.",
            "Component store: dism /Online /Cleanup-Image /StartComponentCleanup",
            "Guided system cleanup: cleanmgr /sageset:1",
        ]
    }
    #[cfg(not(windows))]
    {
        &[
            "Old Rust toolchains: rustup toolchain list, then rustup toolchain uninstall <name>",
            "Simulator runtimes under /Library need sudo, or: xcrun simctl runtime delete --all",
        ]
    }
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// True for files that report a size but hold no local blocks, so counting them
/// would invent reclaimable space that deleting them cannot return.
#[cfg(windows)]
pub fn occupies_no_local_space(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;

    // OneDrive "Files On-Demand" placeholders carry their full logical size in
    // metadata while the bytes live only in the cloud.
    const OFFLINE: u32 = 0x0000_1000;
    const RECALL_ON_OPEN: u32 = 0x0004_0000;
    const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

    meta.file_attributes() & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
}

#[cfg(not(windows))]
pub fn occupies_no_local_space(_meta: &std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetCompressedFileSizeW(path: *const u16, size_high: *mut u32) -> u32;
}

/// Bytes a file actually occupies. Sparse files -- VM disks, emulator images,
/// database files -- advertise the size they could grow to, so trusting the
/// logical length invents reclaimable space that deleting cannot return. An
/// NTFS-compressed file is the same problem in the other direction.
#[cfg(windows)]
pub fn size_on_disk(path: &Path, meta: &std::fs::Metadata) -> u64 {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;

    const SPARSE: u32 = 0x0000_0200;
    const COMPRESSED: u32 = 0x0000_0800;
    const INVALID_FILE_SIZE: u32 = u32::MAX;

    let logical = meta.file_size();
    // The common case pays only an attribute test already in the metadata.
    if meta.file_attributes() & (SPARSE | COMPRESSED) == 0 {
        return logical;
    }

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut high: u32 = 0;
    // SAFETY: `wide` is a NUL-terminated UTF-16 path that outlives the call,
    // and `high` is a live out-param for its duration.
    let low = unsafe { GetCompressedFileSizeW(wide.as_ptr(), &mut high) };

    // Documented failure signal: INVALID_FILE_SIZE plus a set error code.
    if low == INVALID_FILE_SIZE && std::io::Error::last_os_error().raw_os_error() != Some(0) {
        return logical;
    }
    (u64::from(high) << 32) | u64::from(low)
}

/// Bytes a file actually occupies. A sparse VM disk or simulator image
/// advertises the size it could grow to, so the logical length invents
/// reclaimable space that deleting cannot return; `st_blocks` counts the
/// 512-byte units really allocated. Capped at the length so block slack cannot
/// push an ordinary file above the figure Windows reports for it.
#[cfg(not(windows))]
pub fn size_on_disk(_path: &Path, meta: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;

    meta.blocks().saturating_mul(512).min(meta.len())
}

/// Extensions worth flagging as forgotten installers or VM images.
pub fn is_disk_image(ext: &str) -> bool {
    matches!(
        ext,
        "dmg"
            | "iso"
            | "pkg"
            | "img"
            | "vhd"
            | "vhdx"
            | "vmdk"
            | "wim"
            | "esd"
            | "msi"
            | "msu"
            | "msix"
            | "msixbundle"
            | "appx"
            | "appxbundle"
    )
}

/// Roots holding installed software that happens to be laid out like a project.
///
/// An editor extension and a globally installed CLI are both `node_modules`
/// trees next to a `package.json`, so no marker file can tell them apart from a
/// project's dependencies -- but deleting them uninstalls working software
/// instead of freeing a rebuildable cache.
fn installed_software_roots(home: &Path) -> Vec<PathBuf> {
    let mut roots = vec![
        home.join(".vscode").join("extensions"),
        home.join(".vscode-server").join("extensions"),
        home.join(".cursor").join("extensions"),
        home.join(".windsurf").join("extensions"),
    ];
    #[cfg(windows)]
    {
        // npm's global prefix: node_modules here *is* the installed tool set.
        roots.push(roaming_app_data(home).join("npm"));
    }
    #[cfg(not(windows))]
    {
        roots.push(home.join(".npm-global"));
        // bun's global prefix: the node_modules here backs every `bun add -g`
        // binary in ~/.bun/bin. Its sibling install/cache is a real cache.
        roots.push(home.join(".bun/install/global"));
        roots.push(PathBuf::from("/usr/local/lib/node_modules"));
    }
    roots
}

/// Directories `bootstrap` searches for a diskscout checkout.
pub fn workspace_roots(home: &Path) -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        vec![
            home.join("Developer"),
            home.join("source").join("repos"),
            home.join("Documents").join("GitHub"),
            home.join("Projects"),
            home.join("Downloads"),
        ]
    }
    #[cfg(not(windows))]
    {
        vec![home.join("Developer")]
    }
}

// ---------------------------------------------------------------------------
// macOS / Linux
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
pub fn skip_paths(home: &Path) -> Vec<PathBuf> {
    let mut skip = vec![
        PathBuf::from("/System"),
        PathBuf::from("/private"),
        PathBuf::from("/dev"),
        PathBuf::from("/Volumes"),
        PathBuf::from("/proc"),
        home.join("Library"),
        home.join(".Trash"),
    ];
    skip.extend(installed_software_roots(home));
    skip
}

#[cfg(not(windows))]
pub fn system_targets(home: &Path) -> Vec<(PathBuf, Category)> {
    let mut out = Vec::new();

    let support = home.join("Library/Application Support");
    let containers = home.join("Library/Containers");

    let candidates = [
        // Resolve symlink so ~/.build_caches/xcode and the DerivedData symlink
        // don't both end up in the list and get double-counted.
        (
            std::fs::canonicalize(home.join("Library/Developer/Xcode/DerivedData"))
                .unwrap_or_else(|_| home.join("Library/Developer/Xcode/DerivedData")),
            Category::XcodeDerivedData,
        ),
        (
            home.join("Library/Developer/XCTestDevices"),
            Category::XcTestDevices,
        ),
        (
            home.join("Library/Developer/CoreSimulator/Devices"),
            Category::IosSimulators,
        ),
        // Root-owned, so a delete needs sudo; it is reported either way because
        // a stale runtime is the largest thing Xcode leaves outside $HOME.
        (
            PathBuf::from("/Library/Developer/CoreSimulator/Caches"),
            Category::SimulatorRuntimes,
        ),
        (
            PathBuf::from("/Library/Developer/CoreSimulator/Images"),
            Category::SimulatorRuntimes,
        ),
        (
            home.join("Library/Developer/Xcode/iOS DeviceSupport"),
            Category::XcodeDeviceSupport,
        ),
        (
            home.join("Library/Developer/Xcode/watchOS DeviceSupport"),
            Category::XcodeDeviceSupport,
        ),
        (
            home.join("Library/Developer/Xcode/tvOS DeviceSupport"),
            Category::XcodeDeviceSupport,
        ),
        (
            home.join("Library/Developer/Xcode/DocumentationCache"),
            Category::XcodeCaches,
        ),
        (
            home.join("Library/Developer/XcodeBuildMCP/workspaces"),
            Category::XcodeCaches,
        ),
        (support.join("MobileSync/Backup"), Category::IphoneBackups),
        (support.join("FileProvider"), Category::CloudMirror),
        (support.join("Google/DriveFS"), Category::CloudMirror),
        // Claimed ahead of the generic container sweep below so they keep this
        // category rather than the deletable one.
        (
            containers.join("com.apple.photoanalysisd/Data/Library/Caches"),
            Category::ExpensiveCache,
        ),
        (
            containers.join("com.apple.mediaanalysisd/Data/Library/Caches"),
            Category::ExpensiveCache,
        ),
        (
            containers.join("com.apple.texttospeech.SiriAUSP/Data/Library/Caches"),
            Category::ExpensiveCache,
        ),
        (
            home.join("Library/Caches/GeoServices"),
            Category::ExpensiveCache,
        ),
        (
            home.join("Library/Containers/com.docker.docker/Data"),
            Category::DockerData,
        ),
        (home.join("Library/Android/sdk"), Category::AndroidSdk),
        (home.join(".android/avd"), Category::AndroidEmulator),
        (home.join(".npm/_cacache"), Category::NpmCache),
        (home.join(".cache/uv"), Category::UvCache),
        (home.join("Library/Caches/uv"), Category::UvCache),
        (home.join(".cache/pip"), Category::PipCache),
        (home.join(".cache/yarn"), Category::YarnPnpmCache),
        (home.join("Library/Caches/Yarn"), Category::YarnPnpmCache),
        (
            home.join(".local/share/pnpm/store"),
            Category::YarnPnpmCache,
        ),
        (home.join("Library/pnpm/store"), Category::YarnPnpmCache),
        (home.join(".bun/install/cache"), Category::YarnPnpmCache),
        (home.join(".nuget/packages"), Category::NugetCache),
        (home.join(".cargo/registry"), Category::CargoRegistry),
        (home.join(".cargo/git"), Category::CargoRegistry),
        (home.join(".gradle/caches"), Category::GradleCache),
        (home.join(".m2/repository"), Category::MavenCache),
        (home.join("go/pkg/mod"), Category::GoModCache),
    ];

    for (path, cat) in candidates {
        if path.exists() {
            out.push((path, cat));
        }
    }

    push_unclaimed(&mut out, instruments_traces());
    push_unclaimed(&mut out, app_web_caches(&support));
    push_unclaimed(&mut out, container_caches(home));

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

    // One entry per app rather than the whole directory, so the report names
    // what is actually large instead of reporting one opaque total.
    if let Ok(entries) = std::fs::read_dir(home.join("Library/Caches")) {
        let per_app = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .map(|path| (path, Category::MacOsCaches))
            .collect();
        push_unclaimed(&mut out, per_app);
    }

    out
}

/// A broad sweep must not overwrite a category a specific rule already assigned,
/// or a path deliberately held back would come back as deletable.
#[cfg(not(windows))]
fn push_unclaimed(out: &mut Vec<(PathBuf, Category)>, found: Vec<(PathBuf, Category)>) {
    for (path, category) in found {
        if !out.iter().any(|(claimed, _)| claimed == &path) {
            out.push((path, category));
        }
    }
}

/// Identifies the allocation behind a file when more than one path shares it.
/// Cargo and pnpm hard-link aggressively, and counting one blob once per link
/// invents space that deleting the tree cannot return.
#[cfg(not(windows))]
pub fn hard_link_id(meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    (meta.nlink() > 1).then(|| (meta.dev(), meta.ino()))
}

/// std exposes no stable file index on Windows, and reading one costs a handle
/// open per file, so links there are counted separately.
#[cfg(windows)]
pub fn hard_link_id(_meta: &std::fs::Metadata) -> Option<(u64, u64)> {
    None
}

/// The per-user `C` and `T` roots under /var/folders. launchd exports the temp
/// one as TMPDIR; getconf is the documented way to find them when it is not.
#[cfg(not(windows))]
fn darwin_user_dirs() -> Option<(PathBuf, PathBuf)> {
    let anchor = if let Some(tmp) = std::env::var_os("TMPDIR") {
        PathBuf::from(tmp)
    } else {
        let out = std::process::Command::new("getconf")
            .arg("DARWIN_USER_CACHE_DIR")
            .output()
            .ok()?;
        PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
    };

    if !anchor.is_absolute() {
        return None;
    }
    let base = anchor.parent()?;
    Some((base.join("C"), base.join("T")))
}

/// Instruments keeps every recording it has ever taken, and a run that is
/// killed leaves its trace in the temp dir where nothing collects it. Both sit
/// under /var/folders, which the walker never descends into.
#[cfg(not(windows))]
fn instruments_traces() -> Vec<(PathBuf, Category)> {
    let Some((cache, temp)) = darwin_user_dirs() else {
        return Vec::new();
    };
    let mut out = Vec::new();

    let recordings = cache.join("com.apple.dt.Instruments");
    if recordings.is_dir() {
        out.push((recordings, Category::InstrumentsTraces));
    }

    if let Ok(entries) = std::fs::read_dir(&temp) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "ktrace") {
                out.push((path, Category::InstrumentsTraces));
            }
        }
    }

    out
}

/// The directory names Chromium uses for its caches, identical under every
/// Electron app, browser profile and session partition.
#[cfg(not(windows))]
const WEB_CACHE_DIRS: &[&str] = &[
    "Cache",
    "CachedData",
    "Code Cache",
    "DawnCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "GPUCache",
    "GrShaderCache",
    "ShaderCache",
    "Service Worker",
    "component_crx_cache",
];

#[cfg(not(windows))]
fn app_web_caches(support: &Path) -> Vec<(PathBuf, Category)> {
    let mut out = Vec::new();
    // Deep enough for <app>/Partitions/<session>/Cache and <browser>/<profile>/Cache.
    collect_web_caches(support, 3, &mut out);
    out
}

/// Matching on the name finds every Electron app's cache without naming one.
/// A matched directory is never descended into, so nothing is counted twice.
#[cfg(not(windows))]
fn collect_web_caches(dir: &Path, depth: u32, out: &mut Vec<(PathBuf, Category)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };

    for entry in entries.flatten() {
        // read_dir reports a symlink as a symlink, so this cannot follow one
        // out of Application Support or into a cycle.
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        if WEB_CACHE_DIRS
            .iter()
            .any(|name| entry.file_name() == **name)
        {
            out.push((entry.path(), Category::AppWebCache));
        } else if depth > 0 {
            collect_web_caches(&entry.path(), depth - 1, out);
        }
    }
}

/// A sandboxed app caches inside its container rather than in ~/Library/Caches,
/// so scanning that one directory never sees any of it.
#[cfg(not(windows))]
fn container_caches(home: &Path) -> Vec<(PathBuf, Category)> {
    let Ok(entries) = std::fs::read_dir(home.join("Library/Containers")) else {
        return Vec::new();
    };

    entries
        .flatten()
        .map(|entry| entry.path().join("Data/Library/Caches"))
        .filter(|path| path.is_dir())
        .map(|path| (path, Category::ContainerCaches))
        .collect()
}

/// Build caches that live outside any project directory.
#[cfg(not(windows))]
pub fn build_cache_targets(home: &Path) -> Vec<(PathBuf, Category)> {
    // Check both the persistent build cache location and the legacy /tmp path.
    [
        home.join(".build_caches/cargo"),
        PathBuf::from("/tmp/cargo-target"),
    ]
    .into_iter()
    .filter(|p| p.exists())
    .map(|p| (p, Category::RustTarget))
    .collect()
}

/// Files the OS reserves that cannot be deleted directly, reported for
/// information with the command that actually reclaims them.
#[cfg(not(windows))]
pub fn reserved_system_files() -> Vec<(PathBuf, &'static str)> {
    Vec::new()
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn local_app_data(home: &Path) -> PathBuf {
    env_path("LOCALAPPDATA").unwrap_or_else(|| home.join("AppData").join("Local"))
}

#[cfg(windows)]
fn roaming_app_data(home: &Path) -> PathBuf {
    env_path("APPDATA").unwrap_or_else(|| home.join("AppData").join("Roaming"))
}

#[cfg(windows)]
fn windows_dir() -> PathBuf {
    env_path("SystemRoot").unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
}

#[cfg(windows)]
fn program_data() -> PathBuf {
    env_path("ProgramData").unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

#[cfg(windows)]
fn system_drive() -> PathBuf {
    // %SystemDrive% is "C:" with no trailing separator, and a bare "C:" means
    // "the current directory on C:", not its root -- so append the separator.
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    PathBuf::from(format!("{drive}\\"))
}

#[cfg(windows)]
pub fn skip_paths(home: &Path) -> Vec<PathBuf> {
    let local = local_app_data(home);
    let mut skip = vec![
        // Enumerated as fixed targets instead: walking them here would both
        // double-count and cost minutes.
        windows_dir(),
        system_drive().join("$Recycle.Bin"),
        system_drive().join("Windows.old"),
        system_drive().join("$WinREAgent"),
        // Permanently access-denied, one error per entry if walked.
        system_drive().join("System Volume Information"),
        system_drive().join("$GetCurrent"),
        std::env::temp_dir(),
        local.join("Temp"),
        // Zero-byte reparse stubs pointing at Store app installs.
        local.join("Microsoft").join("WindowsApps"),
    ];
    skip.extend(installed_software_roots(home));
    skip
}

#[cfg(windows)]
pub fn system_targets(home: &Path) -> Vec<(PathBuf, Category)> {
    use Category::*;

    let local = local_app_data(home);
    let roaming = roaming_app_data(home);
    let win = windows_dir();
    let shared = program_data();
    let drive = system_drive();

    let candidates = vec![
        // Temp. %TEMP% is what the running session actually writes to.
        (std::env::temp_dir(), WindowsTemp),
        (win.join("Temp"), WindowsTemp),
        // Servicing leftovers. Windows.old only backs the 10-day rollback
        // window and is dead weight once that has passed.
        (
            win.join("SoftwareDistribution").join("Download"),
            WindowsUpdate,
        ),
        (
            win.join("SoftwareDistribution")
                .join("DeliveryOptimization"),
            WindowsUpdate,
        ),
        (
            shared.join("Microsoft").join("Network").join("Downloader"),
            WindowsUpdate,
        ),
        (drive.join("Windows.old"), WindowsOld),
        (drive.join("$WinREAgent"), WindowsOld),
        (drive.join("$Windows.~BT"), WindowsOld),
        (drive.join("$Windows.~WS"), WindowsOld),
        (drive.join("$SysReset"), WindowsOld),
        // Crash dumps and error reports.
        (local.join("CrashDumps"), CrashDumps),
        (win.join("Minidump"), CrashDumps),
        (win.join("LiveKernelReports"), CrashDumps),
        (
            local.join("Microsoft").join("Windows").join("WER"),
            CrashDumps,
        ),
        (
            shared.join("Microsoft").join("Windows").join("WER"),
            CrashDumps,
        ),
        // Shell, shader and web caches.
        (
            local.join("Microsoft").join("Windows").join("INetCache"),
            WindowsCaches,
        ),
        (
            local.join("Microsoft").join("Windows").join("WebCache"),
            WindowsCaches,
        ),
        (local.join("D3DSCache"), WindowsCaches),
        (local.join("NVIDIA").join("DXCache"), WindowsCaches),
        (local.join("NVIDIA").join("GLCache"), WindowsCaches),
        (local.join("AMD").join("DxCache"), WindowsCaches),
        (
            local.join("Microsoft").join("Windows").join("Explorer"),
            ThumbnailCache,
        ),
        // Installer payloads: still needed to repair or uninstall, so these are
        // measured but never marked safe.
        (shared.join("Package Cache"), InstallerCache),
        (win.join("Installer"), InstallerCache),
        // Language and build tool caches.
        (local.join("npm-cache"), NpmCache),
        (home.join(".npm").join("_cacache"), NpmCache),
        (local.join("Yarn").join("Cache"), YarnPnpmCache),
        (
            home.join(".yarn").join("berry").join("cache"),
            YarnPnpmCache,
        ),
        (local.join("pnpm-store"), YarnPnpmCache),
        (local.join("pnpm").join("store"), YarnPnpmCache),
        (local.join("pip").join("Cache"), PipCache),
        (local.join("uv").join("cache"), UvCache),
        (home.join(".nuget").join("packages"), NugetCache),
        (local.join("NuGet").join("v3-cache"), NugetCache),
        (home.join(".cargo").join("registry"), CargoRegistry),
        (home.join(".cargo").join("git"), CargoRegistry),
        (home.join(".gradle").join("caches"), GradleCache),
        (home.join(".m2").join("repository"), MavenCache),
        (home.join("go").join("pkg").join("mod"), GoModCache),
        // IDE caches.
        (local.join("JetBrains"), IdeCache),
        (roaming.join("Code").join("Cache"), IdeCache),
        (roaming.join("Code").join("CachedData"), IdeCache),
        (roaming.join("Code").join("logs"), IdeCache),
        (local.join("Microsoft").join("vscode-cpptools"), IdeCache),
        // Large payloads that hold real state -- measured, never auto-deleted.
        (local.join("Docker").join("wsl"), DockerData),
        (local.join("wsl"), DockerData),
        (local.join("Android").join("Sdk"), AndroidSdk),
        (home.join(".android").join("avd"), AndroidSdk),
    ];

    let mut out: Vec<(PathBuf, Category)> =
        candidates.into_iter().filter(|(p, _)| p.is_dir()).collect();

    // Every fixed drive keeps its own recycle bin; only the system one is
    // predictable, so probe the rest. A: and B: are skipped so a stray floppy
    // letter cannot spin up a drive.
    for letter in 'C'..='Z' {
        let bin = PathBuf::from(format!("{letter}:\\$Recycle.Bin"));
        if bin.is_dir() {
            out.push((bin, RecycleBin));
        }
    }

    for user_data in [
        local.join("Microsoft").join("Edge").join("User Data"),
        local.join("Google").join("Chrome").join("User Data"),
        local
            .join("BraveSoftware")
            .join("Brave-Browser")
            .join("User Data"),
        local.join("Vivaldi").join("User Data"),
        local.join("Chromium").join("User Data"),
        local.join("Opera Software").join("Opera Stable"),
    ] {
        chromium_caches(&user_data, &mut out);
    }
    firefox_caches(
        &local.join("Mozilla").join("Firefox").join("Profiles"),
        &mut out,
    );

    out
}

/// Chromium keeps its evictable caches per profile; the profile directory
/// itself holds history and cookies, so only the cache subdirectories go in.
#[cfg(windows)]
fn chromium_caches(user_data: &Path, out: &mut Vec<(PathBuf, Category)>) {
    const CACHE_DIRS: &[&str] = &[
        "Cache",
        "Code Cache",
        "GPUCache",
        "DawnGraphiteCache",
        "DawnWebGPUCache",
        "Service Worker/CacheStorage",
    ];

    let Ok(entries) = std::fs::read_dir(user_data) else {
        return;
    };

    for entry in entries.flatten() {
        let profile = entry.path();
        if !profile.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_profile =
            name == "Default" || name == "Guest Profile" || name.starts_with("Profile ");
        if !is_profile {
            continue;
        }
        for sub in CACHE_DIRS {
            let path = profile.join(sub);
            if path.is_dir() {
                out.push((path, Category::BrowserCache));
            }
        }
    }

    // Shader caches sit beside the profiles rather than inside them.
    for sub in ["ShaderCache", "GrShaderCache", "GraphiteDawnCache"] {
        let path = user_data.join(sub);
        if path.is_dir() {
            out.push((path, Category::BrowserCache));
        }
    }
}

#[cfg(windows)]
fn firefox_caches(profiles: &Path, out: &mut Vec<(PathBuf, Category)>) {
    let Ok(entries) = std::fs::read_dir(profiles) else {
        return;
    };
    for entry in entries.flatten() {
        let cache = entry.path().join("cache2");
        if cache.is_dir() {
            out.push((cache, Category::BrowserCache));
        }
    }
}

#[cfg(windows)]
pub fn build_cache_targets(home: &Path) -> Vec<(PathBuf, Category)> {
    let mut out = Vec::new();
    if let Some(shared) = env_path("CARGO_TARGET_DIR") {
        out.push(shared);
    }
    out.push(home.join(".build_caches").join("cargo"));
    out.retain(|p| p.is_dir());
    out.into_iter().map(|p| (p, Category::RustTarget)).collect()
}

#[cfg(windows)]
pub fn reserved_system_files() -> Vec<(PathBuf, &'static str)> {
    let drive = system_drive();
    vec![
        (
            drive.join("hiberfil.sys"),
            "disable hibernation to reclaim: powercfg /h off (elevated)",
        ),
        (
            drive.join("pagefile.sys"),
            "resize under System > Advanced > Performance > Virtual memory",
        ),
        (
            drive.join("swapfile.sys"),
            "goes away with the pagefile it backs",
        ),
    ]
    .into_iter()
    .filter(|(p, _)| p.is_file())
    .collect()
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[test]
    fn ordinary_files_report_their_length() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.bin");
        std::fs::write(&path, vec![7u8; 4096]).unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(size_on_disk(&path, &meta), 4096);
    }

    // The case that made a 0.74 GB emulator image read as 512 GB.
    #[cfg(windows)]
    #[test]
    fn a_sparse_file_counts_its_allocation_not_its_hole() {
        const LOGICAL: u64 = 64 * 1_048_576;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sparse.img");
        std::fs::File::create(&path).unwrap();

        let flagged = std::process::Command::new("fsutil")
            .args(["sparse", "setflag"])
            .arg(&path)
            .status()
            .is_ok_and(|s| s.success());
        if !flagged {
            return; // temp dir is not on NTFS; nothing to assert.
        }

        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_len(LOGICAL)
            .unwrap();

        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len(), LOGICAL, "the logical length is the whole 64 MB");
        assert!(
            size_on_disk(&path, &meta) < LOGICAL / 2,
            "the hole must not be counted"
        );
    }

    // APFS makes a truncate-extended file sparse, the shape Docker.raw and the
    // simulator device images have.
    #[cfg(not(windows))]
    #[test]
    fn a_sparse_file_counts_its_allocation_not_its_hole() {
        const LOGICAL: u64 = 64 * 1_048_576;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sparse.img");
        std::fs::File::create(&path)
            .unwrap()
            .set_len(LOGICAL)
            .unwrap();

        let meta = std::fs::metadata(&path).unwrap();
        assert_eq!(meta.len(), LOGICAL, "the logical length is the whole 64 MB");
        assert!(
            size_on_disk(&path, &meta) < LOGICAL / 2,
            "the hole must not be counted"
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn only_a_file_with_more_than_one_link_has_an_identity() {
        let dir = tempfile::tempdir().unwrap();
        let alone = dir.path().join("alone");
        let shared = dir.path().join("shared");
        std::fs::write(&alone, b"a").unwrap();
        std::fs::write(&shared, b"b").unwrap();
        std::fs::hard_link(&shared, dir.path().join("other-name")).unwrap();

        assert!(hard_link_id(&std::fs::metadata(&alone).unwrap()).is_none());
        assert_eq!(
            hard_link_id(&std::fs::metadata(&shared).unwrap()),
            hard_link_id(&std::fs::metadata(dir.path().join("other-name")).unwrap()),
            "both names must resolve to the same allocation"
        );
    }
}

#[cfg(all(test, not(windows)))]
mod target_tests {
    use super::*;

    #[test]
    fn a_web_cache_is_found_at_every_depth_and_never_descended_into() {
        let dir = tempfile::tempdir().unwrap();
        let support = dir.path();
        // The three shapes seen in the wild: an app, a session partition, and
        // a browser profile.
        let plain = support.join("Discord/Cache");
        let partition = support.join("Notion/Partitions/notion/Service Worker");
        let profile = support.join("Google/Chrome/Default/Code Cache");
        for path in [&plain, &partition, &profile] {
            std::fs::create_dir_all(path).unwrap();
        }
        // Chromium nests a directory of the same name inside the cache itself.
        std::fs::create_dir_all(plain.join("Cache")).unwrap();

        let found: Vec<PathBuf> = app_web_caches(support)
            .into_iter()
            .map(|(p, _)| p)
            .collect();

        assert!(found.contains(&plain), "{found:?}");
        assert!(found.contains(&partition), "{found:?}");
        assert!(found.contains(&profile), "{found:?}");
        assert!(
            !found.contains(&plain.join("Cache")),
            "a nested match would be counted twice: {found:?}"
        );
    }

    #[test]
    fn a_containers_cache_is_found_only_at_the_sandbox_path() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let containers = home.join("Library/Containers");
        let real = containers.join("com.apple.Safari/Data/Library/Caches");
        std::fs::create_dir_all(&real).unwrap();
        // A container that keeps no cache contributes nothing.
        std::fs::create_dir_all(containers.join("com.empty.app/Data")).unwrap();

        let found: Vec<PathBuf> = container_caches(home).into_iter().map(|(p, _)| p).collect();

        assert_eq!(found, vec![real]);
    }

    #[test]
    fn a_held_back_path_survives_the_sweep_that_would_reclassify_it() {
        let held = PathBuf::from(
            "/Users/x/Library/Containers/com.apple.photoanalysisd/Data/Library/Caches",
        );
        let ordinary = PathBuf::from("/Users/x/Library/Containers/com.other/Data/Library/Caches");

        let mut out = vec![(held.clone(), Category::ExpensiveCache)];
        push_unclaimed(
            &mut out,
            vec![
                (held.clone(), Category::ContainerCaches),
                (ordinary.clone(), Category::ContainerCaches),
            ],
        );

        assert_eq!(
            out,
            vec![
                (held, Category::ExpensiveCache),
                (ordinary, Category::ContainerCaches),
            ]
        );
    }

    #[test]
    fn the_protected_caches_are_never_auto_deleted() {
        assert!(!Category::ExpensiveCache.safe_to_delete());
        assert!(Category::ExpensiveCache.delete_contents_only());
    }
}
