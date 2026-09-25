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
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

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
            wire_zsh_completions(&home, &zsh_rc_dir(&home))?;
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
    for exe in ["pwsh", "powershell"] {
        let out = Command::new(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", "$PROFILE"])
            .output();
        if let Ok(out) = out
            && out.status.success()
        {
            let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !path.is_empty() {
                return PathBuf::from(path);
            }
        }
    }
    home.join("Documents")
        .join("WindowsPowerShell")
        .join("Microsoft.PowerShell_profile.ps1")
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
// zsh completion wiring
// ---------------------------------------------------------------------------

const ZSH_FPATH_LINE: &str = "fpath=(~/.zsh/completions $fpath)";

/// `~/.zshenv` is read before every other startup file, so an `fpath` entry set
/// there is in place before any `compinit` runs, whatever framework the user has.
/// Shared with every tool that installs into `~/.zsh/completions`.
const ZSHENV_BLOCK: &str = "# Completion scripts tools install into ~/.zsh/completions, on fpath before any compinit.\nfpath=(~/.zsh/completions $fpath)\n";

/// Appended only when nothing in the startup files runs `compinit`, so it is the
/// one `compinit` rather than an extra one.
const ZSHRC_COMPINIT_BLOCK: &str = "# Loads completions, including ~/.zsh/completions (added to fpath in ~/.zshenv).\nautoload -Uz compinit && compinit\n";

/// What older versions appended: one block per tool, each running its own
/// `compinit`, which rebuilds the completion cache on every shell start.
const ZSHRC_LEGACY_BLOCK: &str = "# diskscout completions\nfpath=(~/.zsh/completions $fpath)\nautoload -U compinit && compinit\n";

const ZSH_PROBE_TAG: &str = "__diskscout_completion_probe__";
const ZSH_PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// What a fresh interactive zsh reports once it has run the startup files.
#[derive(Debug, PartialEq, Eq)]
enum ZshCompletions {
    /// `compinit` ran with `~/.zsh/completions` on `fpath`, so `_diskscout` loads.
    Loaded,
    /// `compinit` ran before `~/.zsh/completions` was on `fpath`.
    DirMissing,
    /// Nothing in the startup files runs `compinit`.
    NoCompinit,
}

fn zsh_rc_dir(home: &Path) -> PathBuf {
    std::env::var_os("ZDOTDIR")
        .filter(|dir| !dir.is_empty())
        .map_or_else(|| home.to_path_buf(), PathBuf::from)
}

/// Makes zsh load `~/.zsh/completions` without adding a `compinit` to a startup
/// that already runs one. Every decision comes from what a real zsh reports
/// rather than from marker text, so it converges on a working setup whatever
/// state the startup files are in, and a probe that cannot run leaves them
/// untouched.
fn wire_zsh_completions(home: &Path, rc_dir: &Path) -> Result<(), String> {
    let legacy_rc = home.join(".zshrc");
    let has_legacy = std::fs::read_to_string(&legacy_rc)
        .unwrap_or_default()
        .contains(ZSHRC_LEGACY_BLOCK);

    let state = match probe_zsh(home) {
        Err(reason) => {
            zsh_manual_steps(&reason, has_legacy.then_some(legacy_rc.as_path()));
            return Ok(());
        }
        Ok(_) if has_legacy => {
            remove_block(&legacy_rc, ZSHRC_LEGACY_BLOCK)?;
            row(
                "rc file",
                format!(
                    "{} (removed the old per-tool compinit block)",
                    legacy_rc.display()
                )
                .dimmed(),
            );
            match probe_zsh(home) {
                Ok(state) => state,
                Err(reason) => {
                    zsh_manual_steps(&reason, None);
                    return Ok(());
                }
            }
        }
        Ok(state) => state,
    };

    let zshenv = home.join(".zshenv");
    let zshrc = rc_dir.join(".zshrc");
    match state {
        ZshCompletions::Loaded => {
            row("rc file", "zsh already loads ~/.zsh/completions".dimmed());
            return Ok(());
        }
        ZshCompletions::DirMissing => {
            append_unless_present(&zshenv, ZSH_FPATH_LINE, ZSHENV_BLOCK)?;
            row("zshenv", zshenv.display().to_string().cyan());
        }
        ZshCompletions::NoCompinit => {
            append_unless_present(&zshenv, ZSH_FPATH_LINE, ZSHENV_BLOCK)?;
            append_unless_present(&zshrc, ZSHRC_COMPINIT_BLOCK, ZSHRC_COMPINIT_BLOCK)?;
            row("zshenv", zshenv.display().to_string().cyan());
            row("rc file", zshrc.display().to_string().cyan());
        }
    }

    match probe_zsh(home) {
        Ok(ZshCompletions::Loaded) => {}
        Ok(state) => zsh_manual_steps(&format!("zsh still reports {state:?} after the edit"), None),
        Err(reason) => zsh_manual_steps(&reason, None),
    }
    Ok(())
}

/// Runs the startup files in a fresh interactive zsh and reports what they did;
/// the error is only ever the reason in a printed notice. `HOME` is passed
/// explicitly so tests can point it at a fixture, and the calling shell's `FPATH`
/// and `ZDOTDIR` are dropped so they cannot stand in for what a new terminal's
/// startup files set.
fn probe_zsh(home: &Path) -> Result<ZshCompletions, String> {
    let mut child = Command::new("zsh")
        .arg("-i")
        .arg("-c")
        .arg(format!(
            "print -r -- \"{ZSH_PROBE_TAG} ${{+functions[compdef]}} ${{+_comps[diskscout]}}\""
        ))
        .env("HOME", home)
        .env_remove("FPATH")
        .env_remove("ZDOTDIR")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("could not run zsh: {e}"))?;
    let Some(mut stdout) = child.stdout.take() else {
        return Err("zsh stdout was not captured".to_owned());
    };

    // A background job started by the startup files can hold stdout open after
    // zsh exits, so the read gets a deadline rather than a blocking join.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.read_to_end(&mut out);
        let _ = tx.send(out);
    });
    let out = rx.recv_timeout(ZSH_PROBE_TIMEOUT);
    if out.is_err() {
        let _ = child.kill();
    }
    let _ = child.wait();
    let out =
        out.map_err(|_| format!("zsh did not finish within {}s", ZSH_PROBE_TIMEOUT.as_secs()))?;
    parse_zsh_probe(&String::from_utf8_lossy(&out))
}

fn parse_zsh_probe(out: &str) -> Result<ZshCompletions, String> {
    let Some(line) = out
        .lines()
        .rev()
        .find_map(|line| line.strip_prefix(ZSH_PROBE_TAG))
    else {
        return Err("zsh exited before it ran the probe command".to_owned());
    };
    let mut fields = line.split_whitespace();
    match (fields.next(), fields.next()) {
        (Some("1"), Some("1")) => Ok(ZshCompletions::Loaded),
        (Some("1"), Some("0")) => Ok(ZshCompletions::DirMissing),
        (Some("0"), Some(_)) => Ok(ZshCompletions::NoCompinit),
        _ => Err(format!("unexpected probe output: {line:?}")),
    }
}

fn zsh_manual_steps(reason: &str, legacy_rc: Option<&Path>) {
    row("rc file", format!("not changed: {reason}").yellow());
    row(
        "",
        "to load ~/.zsh/completions, add this line to ~/.zshenv and open a new shell:",
    );
    row("", ZSH_FPATH_LINE.cyan());
    if let Some(rc) = legacy_rc {
        row(
            "",
            format!(
                "then delete the `# diskscout completions` block from {} - it runs a second compinit",
                rc.display()
            ),
        );
    }
}

fn row(label: &str, value: impl std::fmt::Display) {
    println!("{}  {value}", format!("{label:<14}").dimmed());
}

/// Appends `block` unless `needle` is already in the file.
fn append_unless_present(path: &Path, needle: &str, block: &str) -> Result<(), String> {
    if std::fs::read_to_string(path)
        .unwrap_or_default()
        .contains(needle)
    {
        return Ok(());
    }
    append_line(path, &format!("\n{block}"))
}

/// Removes one copy of `block` and the blank line the old append put before it.
/// Writes in place rather than renaming over the file, so a `.zshrc` that is a
/// symlink into a dotfiles repo stays a symlink.
fn remove_block(path: &Path, block: &str) -> Result<(), String> {
    let text =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let spaced = format!("\n{block}");
    let updated = if text.contains(&spaced) {
        text.replacen(&spaced, "", 1)
    } else {
        text.replacen(block, "", 1)
    };
    std::fs::write(path, updated).map_err(|e| format!("write {}: {e}", path.display()))
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
    use std::fs;

    #[test]
    fn the_probe_line_is_found_past_whatever_the_startup_files_print() {
        let out = format!("Restored session: today\n{ZSH_PROBE_TAG} 1 0\n");
        assert_eq!(parse_zsh_probe(&out), Ok(ZshCompletions::DirMissing));
        assert_eq!(
            parse_zsh_probe(&format!("{ZSH_PROBE_TAG} 1 1\n")),
            Ok(ZshCompletions::Loaded)
        );
        assert_eq!(
            parse_zsh_probe(&format!("{ZSH_PROBE_TAG} 0 0\n")),
            Ok(ZshCompletions::NoCompinit)
        );
    }

    #[test]
    fn a_probe_that_never_ran_or_printed_garbage_is_an_error() {
        assert!(parse_zsh_probe("some rc output\n").is_err());
        assert!(parse_zsh_probe(&format!("{ZSH_PROBE_TAG} yes\n")).is_err());
    }

    #[test]
    fn removing_the_legacy_block_takes_the_blank_line_the_old_append_added() {
        let dir = tempfile::tempdir().expect("temp dir");
        let rc = dir.path().join(".zshrc");
        fs::write(
            &rc,
            format!("export A=1\n\n{ZSHRC_LEGACY_BLOCK}export B=2\n"),
        )
        .expect("write");
        remove_block(&rc, ZSHRC_LEGACY_BLOCK).expect("remove");
        assert_eq!(
            fs::read_to_string(&rc).expect("read"),
            "export A=1\nexport B=2\n"
        );
    }

    fn zsh_available() -> bool {
        Command::new("zsh")
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success())
    }

    /// A HOME holding `_diskscout` in `~/.zsh/completions`, with Debian's global
    /// compinit switched off so each case alone decides whether compinit runs.
    fn zsh_home(zshrc: &str) -> tempfile::TempDir {
        let home = tempfile::tempdir().expect("temp dir");
        let completions = home.path().join(".zsh/completions");
        fs::create_dir_all(&completions).expect("mkdir");
        fs::write(
            completions.join("_diskscout"),
            "#compdef diskscout\n_diskscout() {}\n",
        )
        .expect("write");
        fs::write(home.path().join(".zshenv"), "skip_global_compinit=1\n").expect("write");
        fs::write(home.path().join(".zshrc"), zshrc).expect("write");
        home
    }

    fn read(home: &Path, file: &str) -> String {
        fs::read_to_string(home.join(file)).expect("read")
    }

    #[test]
    fn the_probe_tells_the_three_startup_shapes_apart() {
        if !zsh_available() {
            return;
        }
        let none = zsh_home("");
        assert_eq!(probe_zsh(none.path()), Ok(ZshCompletions::NoCompinit));
        let late = zsh_home("autoload -Uz compinit && compinit\n");
        assert_eq!(probe_zsh(late.path()), Ok(ZshCompletions::DirMissing));
        let early = zsh_home(&format!(
            "{ZSH_FPATH_LINE}\nautoload -Uz compinit && compinit\n"
        ));
        assert_eq!(probe_zsh(early.path()), Ok(ZshCompletions::Loaded));
    }

    #[test]
    fn a_startup_that_already_runs_compinit_gets_no_second_one() {
        if !zsh_available() {
            return;
        }
        let home = zsh_home("autoload -Uz compinit && compinit\n");
        wire_zsh_completions(home.path(), home.path()).expect("wire");
        assert_eq!(
            read(home.path(), ".zshrc"),
            "autoload -Uz compinit && compinit\n"
        );
        assert!(read(home.path(), ".zshenv").contains(ZSH_FPATH_LINE));
        assert_eq!(probe_zsh(home.path()), Ok(ZshCompletions::Loaded));
    }

    #[test]
    fn a_startup_without_compinit_gets_exactly_one_and_a_rerun_changes_nothing() {
        if !zsh_available() {
            return;
        }
        let home = zsh_home("");
        wire_zsh_completions(home.path(), home.path()).expect("wire");
        assert_eq!(probe_zsh(home.path()), Ok(ZshCompletions::Loaded));
        assert_eq!(
            read(home.path(), ".zshrc").matches("compinit &&").count(),
            1
        );

        let (zshrc, zshenv) = (read(home.path(), ".zshrc"), read(home.path(), ".zshenv"));
        wire_zsh_completions(home.path(), home.path()).expect("wire");
        assert_eq!(read(home.path(), ".zshrc"), zshrc);
        assert_eq!(read(home.path(), ".zshenv"), zshenv);
    }

    #[test]
    fn the_legacy_per_tool_block_is_replaced_by_the_zshenv_line() {
        if !zsh_available() {
            return;
        }
        let home = zsh_home(&format!(
            "autoload -Uz compinit && compinit\n\n{ZSHRC_LEGACY_BLOCK}"
        ));
        wire_zsh_completions(home.path(), home.path()).expect("wire");
        assert_eq!(
            read(home.path(), ".zshrc"),
            "autoload -Uz compinit && compinit\n"
        );
        assert!(read(home.path(), ".zshenv").contains(ZSH_FPATH_LINE));
        assert_eq!(probe_zsh(home.path()), Ok(ZshCompletions::Loaded));
    }

    #[test]
    fn a_working_setup_is_left_exactly_as_it_was() {
        if !zsh_available() {
            return;
        }
        let zshrc = format!("{ZSH_FPATH_LINE}\nautoload -Uz compinit && compinit\n");
        let home = zsh_home(&zshrc);
        wire_zsh_completions(home.path(), home.path()).expect("wire");
        assert_eq!(read(home.path(), ".zshrc"), zshrc);
        assert_eq!(read(home.path(), ".zshenv"), "skip_global_compinit=1\n");
    }
}
