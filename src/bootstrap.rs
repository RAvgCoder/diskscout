//! Hidden `__bootstrap` command: install diskscout and wire up shell completions.
//!
//! Mirrors the company `cex __bootstrap` flow (cargo install + completion
//! generation + rc-file patch) but resolves its source checkout from the
//! `DISKSCOUT_SRC` environment variable rather than a persisted JSON store.
//!
//! Source resolution order:
//!   1. `DISKSCOUT_SRC` if set and pointing at a diskscout checkout
//!   2. auto-discovery scan under the platform's usual checkout roots
//!   3. the current working directory
//!
//! Once resolved, the path is written back as an `export DISKSCOUT_SRC=...`
//! line in the shell rc file so the next run skips discovery. That export is
//! the env-var analog of the store.json key `cex` persists.

use clap::{Args, CommandFactory};
use clap_complete::{Shell as CompShell, generate};
use colored::Colorize;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Cli;

const SRC_ENV: &str = "DISKSCOUT_SRC";

#[derive(Debug, Args)]
pub struct BootstrapArgs {
    /// Write or refresh shell completions without reinstalling the binary.
    #[arg(long)]
    pub only_completions: bool,

    /// Build with cargo's debug profile: faster link, slower binary.
    #[arg(long, short = 'd')]
    pub debug: bool,
}

pub fn run(args: &BootstrapArgs) -> Result<(), String> {
    println!("{}", "diskscout __bootstrap".bold());
    println!();

    if args.only_completions {
        write_completions()?;
        persist_source_env(None);
        println!();
        println!("{}", "done".green().bold());
        println!(
            "  Run {} to load completions now.",
            Shell::detect().reload_hint().cyan()
        );
        return Ok(());
    }

    let source = resolve_source()?;
    println!(
        "{}  {}",
        format!("{:<14}", "source").dimmed(),
        source.display().to_string().cyan()
    );

    install_binary(&source, args.debug)?;
    write_completions()?;
    persist_source_env(Some(&source));

    println!();
    println!("{}", "setup complete".green().bold());
    println!(
        "  Run {} to load completions now.",
        Shell::detect().reload_hint().cyan()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Source resolution (env var, not store.json)
// ---------------------------------------------------------------------------

fn resolve_source() -> Result<PathBuf, String> {
    if let Some(raw) = std::env::var_os(SRC_ENV) {
        let path = expand_tilde(&raw.to_string_lossy());
        if is_diskscout_workspace(&path) {
            return Ok(path);
        }
        eprintln!(
            "  {} ${SRC_ENV} = '{}' is not a diskscout checkout -- ignoring",
            "note:".dimmed(),
            path.display()
        );
    }

    if let Some(found) = discover_workspace() {
        return Ok(found);
    }

    let cwd = std::env::current_dir().map_err(|e| format!("cannot read cwd: {e}"))?;
    if is_diskscout_workspace(&cwd) {
        return Ok(cwd);
    }

    let example = match Shell::detect() {
        Shell::Pwsh => format!("$env:{SRC_ENV} = \"$HOME\\Developer\\diskscout\""),
        _ => format!("export {SRC_ENV}=~/Developer/diskscout"),
    };
    Err(format!(
        "no diskscout checkout found.\n  Set {SRC_ENV} to the repo path, e.g.\n    {example}"
    ))
}

const SKIP_DIRS: &[&str] = &["node_modules", "target", "build", "dist", ".git"];

fn discover_workspace() -> Option<PathBuf> {
    let home = crate::platform::home_dir()?;
    let mut hits = Vec::new();
    for root in crate::platform::workspace_roots(&home) {
        if root.is_dir() {
            scan(&root, 0, 4, &mut hits);
        }
    }
    hits.sort();
    hits.into_iter().next()
}

fn scan(dir: &Path, depth: u32, max: u32, hits: &mut Vec<PathBuf>) {
    if depth > max {
        return;
    }
    if is_diskscout_workspace(dir) {
        hits.push(dir.canonicalize().unwrap_or_else(|_| dir.to_owned()));
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || SKIP_DIRS.contains(&name.as_ref()) {
            continue;
        }
        scan(&path, depth + 1, max, hits);
    }
}

fn is_diskscout_workspace(path: &Path) -> bool {
    let manifest = path.join("Cargo.toml");
    std::fs::read_to_string(manifest).is_ok_and(|s| s.contains("name = \"diskscout\""))
}

// ---------------------------------------------------------------------------
// Binary install
// ---------------------------------------------------------------------------

fn install_binary(source: &Path, debug: bool) -> Result<(), String> {
    let profile = if debug {
        "debug-build"
    } else {
        "release-build"
    };
    println!(
        "{}  {}",
        format!("{:<14}", "binary").dimmed(),
        format!("installing ({profile})...").dimmed()
    );

    let mut cmd = Command::new("cargo");
    cmd.arg("install").arg("--path").arg(source).arg("--force");
    if debug {
        cmd.arg("--debug");
    }

    let status = cmd
        .status()
        .map_err(|e| format!("failed to spawn cargo: {e}"))?;
    if !status.success() {
        return Err(format!(
            "cargo install failed (exit {:?}); run `cargo install --path {}` manually",
            status.code(),
            source.display()
        ));
    }

    println!(
        "{}  {}",
        format!("{:<14}", "binary").dimmed(),
        "installed".green()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Shell completions + rc patch
// ---------------------------------------------------------------------------

enum Shell {
    Zsh,
    Bash,
    Fish,
    Pwsh,
}

impl Shell {
    fn detect() -> Self {
        // Windows has no $SHELL; PowerShell is the shell that has completions
        // worth generating, and cmd.exe has none at all.
        if cfg!(windows) {
            return Shell::Pwsh;
        }
        let shell = std::env::var("SHELL").unwrap_or_default();
        if shell.contains("zsh") {
            Shell::Zsh
        } else if shell.contains("fish") {
            Shell::Fish
        } else {
            Shell::Bash
        }
    }

    /// What to tell the user to run so the new completions load now.
    fn reload_hint(&self) -> &'static str {
        match self {
            Shell::Pwsh => ". $PROFILE",
            _ => "exec $SHELL",
        }
    }
}

fn write_completions() -> Result<(), String> {
    let home = home_dir()?;
    let mut cmd = Cli::command();

    match Shell::detect() {
        Shell::Zsh => {
            let dir = home.join(".zsh/completions");
            write_completion_file(&dir, "_diskscout", CompShell::Zsh, &mut cmd)?;
            patch_rc(
                &home.join(".zshrc"),
                "# diskscout completions",
                "# diskscout completions\nfpath=(~/.zsh/completions $fpath)\nautoload -U compinit && compinit\n",
            )?;
        }
        Shell::Bash => {
            let dir = home.join(".bash_completion.d");
            write_completion_file(&dir, "diskscout", CompShell::Bash, &mut cmd)?;
            let rc = if home.join(".bash_profile").exists() {
                home.join(".bash_profile")
            } else {
                home.join(".bashrc")
            };
            patch_rc(
                &rc,
                "# diskscout completions",
                "# diskscout completions\n[ -f ~/.bash_completion.d/diskscout ] && source ~/.bash_completion.d/diskscout\n",
            )?;
        }
        Shell::Fish => {
            let dir = home.join(".config/fish/completions");
            write_completion_file(&dir, "diskscout.fish", CompShell::Fish, &mut cmd)?;
            // fish auto-loads this directory; no rc edit needed.
        }
        Shell::Pwsh => {
            let profile = powershell_profile(&home);
            let dir = profile
                .parent()
                .map_or_else(|| home.clone(), Path::to_path_buf)
                .join("Completions");
            write_completion_file(&dir, "diskscout.ps1", CompShell::PowerShell, &mut cmd)?;
            patch_rc(
                &profile,
                "# diskscout completions",
                &format!(
                    "# diskscout completions\n. \"{}\"\n",
                    dir.join("diskscout.ps1").display()
                ),
            )?;
        }
    }
    Ok(())
}

/// Asks PowerShell where `$PROFILE` is, because OneDrive folder redirection
/// moves Documents and the path cannot be assumed. Falls back to the default
/// location for whichever edition answered.
#[cfg(windows)]
fn powershell_profile(home: &Path) -> PathBuf {
    // PowerShell encodes a redirected stdout with the console codepage it
    // inherited, not UTF-8, and `Command::output` always redirects. When
    // OneDrive has redirected Documents to a localised name that codepage
    // cannot represent -- `المستندات` on an Arabic profile, and the same for
    // CJK or Cyrillic -- each such character arrives as `?`. That is not a
    // legal Windows path character, so the completions mkdir failed with "The
    // filename, directory name, or volume label syntax is incorrect" (os error
    // 123) and the install stopped one step short of done. Pinning the child's
    // output encoding is what makes the answer survive the pipe.
    const QUERY: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; $PROFILE";

    for exe in ["pwsh", "powershell"] {
        let out = Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", QUERY])
            .output();
        if let Ok(out) = out
            && out.status.success()
            && let Some(path) = parse_profile_answer(&out.stdout)
        {
            return path;
        }
    }
    home.join("Documents")
        .join("WindowsPowerShell")
        .join("Microsoft.PowerShell_profile.ps1")
}

/// Decodes a `$PROFILE` answer, rejecting one that did not survive the pipe.
///
/// A lossy encoding is not a recoverable error: the characters are already
/// gone, and the mangling leaves behind a string that still looks like a path.
/// Taking it would create a directory named after the damage and write
/// completions somewhere PowerShell never loads them, so a mangled answer is
/// refused and the caller falls through to the next candidate instead.
#[cfg_attr(not(windows), allow(dead_code))]
fn parse_profile_answer(stdout: &[u8]) -> Option<PathBuf> {
    let answer = String::from_utf8_lossy(stdout);
    let answer = answer.trim();
    // `?` and friends are the codepage substitution; U+FFFD is what
    // `from_utf8_lossy` leaves when the bytes were some other encoding
    // entirely. Neither can appear in a path PowerShell actually reported.
    if answer.is_empty() || answer.contains(['?', '*', '<', '>', '|', '"', '\u{FFFD}']) {
        return None;
    }
    Some(PathBuf::from(answer))
}

#[cfg(not(windows))]
fn powershell_profile(home: &Path) -> PathBuf {
    home.join(".config")
        .join("powershell")
        .join("Microsoft.PowerShell_profile.ps1")
}

fn write_completion_file(
    dir: &Path,
    file: &str,
    shell: CompShell,
    cmd: &mut clap::Command,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let path = dir.join(file);
    let mut buf = Vec::new();
    generate(shell, cmd, "diskscout", &mut buf);
    std::fs::write(&path, &buf).map_err(|e| format!("write {}: {e}", path.display()))?;
    println!(
        "{}  {}",
        format!("{:<14}", "completions").dimmed(),
        path.display().to_string().cyan()
    );
    Ok(())
}

fn patch_rc(rc: &Path, marker: &str, block: &str) -> Result<(), String> {
    let existing = std::fs::read_to_string(rc).unwrap_or_default();
    if existing.contains(marker) {
        println!(
            "{}  {} (already configured)",
            format!("{:<14}", "rc file").dimmed(),
            rc.display().to_string().dimmed()
        );
        return Ok(());
    }
    append_line(rc, &format!("\n{block}"))?;
    println!(
        "{}  {}",
        format!("{:<14}", "rc file").dimmed(),
        rc.display().to_string().cyan()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// Env-var persistence (the store.json replacement)
// ---------------------------------------------------------------------------

fn persist_source_env(source: Option<&Path>) {
    let Some(source) = source else {
        return;
    };
    let Ok(home) = home_dir() else {
        return;
    };
    let shell = Shell::detect();
    let rc = match shell {
        Shell::Pwsh => powershell_profile(&home),
        Shell::Fish => home.join(".config/fish/config.fish"),
        _ if home.join(".zshrc").exists() => home.join(".zshrc"),
        _ => home.join(".bashrc"),
    };

    let marker = match shell {
        Shell::Pwsh => format!("$env:{SRC_ENV} ="),
        _ => format!("export {SRC_ENV}="),
    };
    let existing = std::fs::read_to_string(&rc).unwrap_or_default();
    if existing.contains(&marker) {
        return;
    }
    let line = match shell {
        Shell::Pwsh => format!("\n$env:{SRC_ENV} = \"{}\"\n", source.display()),
        Shell::Fish => format!("\nset -gx {SRC_ENV} \"{}\"\n", source.display()),
        _ => format!("\nexport {SRC_ENV}=\"{}\"\n", source.display()),
    };
    if append_line(&rc, &line).is_ok() {
        println!(
            "{}  {}={}",
            format!("{:<14}", "env").dimmed(),
            SRC_ENV.cyan(),
            source.display().to_string().dimmed()
        );
    }
}

// ---------------------------------------------------------------------------
// Small helpers
// ---------------------------------------------------------------------------

fn append_line(path: &Path, text: &str) -> Result<(), String> {
    // A PowerShell profile often has no directory yet on a fresh install.
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir {}: {e}", parent.display()))?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn home_dir() -> Result<PathBuf, String> {
    crate::platform::home_dir().ok_or_else(|| format!("{} is not set", crate::platform::HOME_VAR))
}

fn expand_tilde(s: &str) -> PathBuf {
    match s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
        Some(rest) => {
            crate::platform::home_dir().map_or_else(|| PathBuf::from(s), |h| h.join(rest))
        }
        None => PathBuf::from(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ARABIC_DOCUMENTS: &str = "المستندات";

    #[test]
    fn profile_answer_keeps_a_non_ascii_documents_folder() {
        let answer = format!(
            "C:\\Users\\u\\OneDrive\\{ARABIC_DOCUMENTS}\\PowerShell\\Microsoft.PowerShell_profile.ps1\r\n"
        );
        let got = parse_profile_answer(answer.as_bytes()).expect("a clean answer is accepted");
        assert!(got.to_string_lossy().contains(ARABIC_DOCUMENTS));
    }

    /// Pins the install failure: PowerShell encoded the pipe with the console
    /// codepage, an Arabic Documents folder came back as `?????????`, and the
    /// completions mkdir died with os error 123.
    #[test]
    fn profile_answer_mangled_by_the_console_codepage_is_rejected() {
        let mangled =
            "C:\\Users\\u\\OneDrive\\?????????\\PowerShell\\Microsoft.PowerShell_profile.ps1\r\n";
        assert!(parse_profile_answer(mangled.as_bytes()).is_none());
    }

    /// The other half of the same failure: bytes in some non-UTF-8 encoding
    /// rather than substituted characters. Here CP1256 for `المستندات`.
    #[test]
    fn profile_answer_that_is_not_utf8_is_rejected() {
        let mut bytes = b"C:\\Users\\u\\OneDrive\\".to_vec();
        bytes.extend_from_slice(&[0xc7, 0xe1, 0xe3, 0xd3, 0xca, 0xe4, 0xcf, 0xc7, 0xca]);
        bytes.extend_from_slice(b"\\PowerShell\\Microsoft.PowerShell_profile.ps1\r\n");
        assert!(parse_profile_answer(&bytes).is_none());
    }

    #[test]
    fn empty_profile_answer_is_rejected() {
        assert!(parse_profile_answer(b"  \r\n").is_none());
        assert!(parse_profile_answer(b"").is_none());
    }
}
