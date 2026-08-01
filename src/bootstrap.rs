//! Hidden `__bootstrap` command: install diskscout and wire up shell completions.
//!
//! Mirrors the company `cex __bootstrap` flow (cargo install + completion
//! generation + rc-file patch) but resolves its source checkout from the
//! `DISKSCOUT_SRC` environment variable rather than a persisted JSON store.
//!
//! Source resolution order:
//!   1. `DISKSCOUT_SRC` if set and pointing at a diskscout checkout
//!   2. auto-discovery scan under `~/Developer`
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
        println!("  Run {} to load completions now.", "exec $SHELL".cyan());
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
    println!("  Run {} to load completions now.", "exec $SHELL".cyan());
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

    Err(format!(
        "no diskscout checkout found.\n  Set {SRC_ENV} to the repo path, e.g.\n    export {SRC_ENV}=~/Developer/diskscout"
    ))
}

const SKIP_DIRS: &[&str] = &["node_modules", "target", "build", "dist", ".git"];

fn discover_workspace() -> Option<PathBuf> {
    let dev = PathBuf::from(std::env::var_os("HOME")?).join("Developer");
    if !dev.is_dir() {
        return None;
    }
    let mut hits = Vec::new();
    scan(&dev, 0, 4, &mut hits);
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
}

impl Shell {
    fn detect() -> Self {
        let shell = std::env::var("SHELL").unwrap_or_default();
        if shell.contains("zsh") {
            Shell::Zsh
        } else if shell.contains("fish") {
            Shell::Fish
        } else {
            Shell::Bash
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
    }
    Ok(())
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
    let rc = if matches!(Shell::detect(), Shell::Fish) {
        home.join(".config/fish/config.fish")
    } else if home.join(".zshrc").exists() {
        home.join(".zshrc")
    } else {
        home.join(".bashrc")
    };

    let existing = std::fs::read_to_string(&rc).unwrap_or_default();
    if existing.contains(&format!("export {SRC_ENV}=")) {
        return;
    }
    let line = if matches!(Shell::detect(), Shell::Fish) {
        format!("\nset -gx {SRC_ENV} \"{}\"\n", source.display())
    } else {
        format!("\nexport {SRC_ENV}=\"{}\"\n", source.display())
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
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open {}: {e}", path.display()))?;
    file.write_all(text.as_bytes())
        .map_err(|e| format!("write {}: {e}", path.display()))
}

fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "$HOME is not set".to_owned())
}

fn expand_tilde(s: &str) -> PathBuf {
    match s.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map_or_else(|| PathBuf::from(s), |h| PathBuf::from(h).join(rest)),
        None => PathBuf::from(s),
    }
}
