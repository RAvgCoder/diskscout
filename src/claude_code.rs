use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::platform;

/// Claude Code's per-user temp folders that exist on this machine.
pub fn temp_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = platform::claude_temp_bases()
        .iter()
        .flat_map(|base| subdirectories(base))
        .filter(|dir| {
            dir.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(is_temp_dir_name)
        })
        .map(|dir| std::fs::canonicalize(&dir).unwrap_or(dir))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Claude Code names the folder `claude-<uid>`, and `claude-0` where the OS has
/// no uid.
pub fn is_temp_dir_name(name: &str) -> bool {
    name.strip_prefix("claude-")
        .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()))
}

/// One entry per session folder, plus anything else left at the top level.
pub fn temp_entries(temp_dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for child in subdirectories(temp_dir) {
        // A project folder is the project's path with every separator made a
        // dash, so it always starts with one; its children are sessions.
        let is_project = child
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('-'));
        if is_project {
            out.extend(subdirectories(&child));
        } else {
            out.push(child);
        }
    }
    out
}

/// Which sessions are running, from the registry each interactive Claude Code
/// process keeps. Headless and background runs do not register.
pub enum Sessions {
    Running(HashSet<String>),
    Unreadable,
}

impl Sessions {
    pub fn load(home: &Path) -> Sessions {
        let config = std::env::var_os("CLAUDE_CONFIG_DIR")
            .filter(|v| !v.is_empty())
            .map_or_else(|| home.join(".claude"), PathBuf::from);
        let Ok(entries) = std::fs::read_dir(config.join("sessions")) else {
            return Sessions::Unreadable;
        };
        let Some(live_pids) = platform::running_pids() else {
            return Sessions::Unreadable;
        };

        let mut running = HashSet::new();
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(json) = std::fs::read_to_string(&path) else {
                return Sessions::Unreadable;
            };
            let (Some(pid), Some(id)) =
                (json_number(&json, "pid"), json_string(&json, "sessionId"))
            else {
                return Sessions::Unreadable;
            };
            if live_pids.contains(&pid) {
                running.insert(id.to_owned());
            }
        }
        Sessions::Running(running)
    }

    /// Only what the registry confirms: false for anything it cannot place.
    pub fn is_running(&self, folder: &Path) -> bool {
        let name = folder.file_name().and_then(|name| name.to_str());
        match self {
            Sessions::Running(ids) => name.is_some_and(|name| ids.contains(name)),
            Sessions::Unreadable => false,
        }
    }

    /// A folder not named like a session could belong to anything, so it
    /// counts as possibly running.
    pub fn may_be_running(&self, folder: &Path) -> bool {
        let Some(name) = folder
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| is_session_id(name))
        else {
            return true;
        };
        match self {
            Sessions::Running(ids) => ids.contains(name),
            Sessions::Unreadable => true,
        }
    }
}

fn is_session_id(name: &str) -> bool {
    let groups: Vec<&str> = name.split('-').collect();
    groups.iter().map(|g| g.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|g| g.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn json_value<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let at = json.find(&format!("\"{key}\""))? + key.len() + 2;
    let rest = json[at..].trim_start().strip_prefix(':')?;
    Some(rest.trim_start())
}

fn json_string<'a>(json: &'a str, key: &str) -> Option<&'a str> {
    let rest = json_value(json, key)?.strip_prefix('"')?;
    Some(&rest[..rest.find('"')?])
}

fn json_number(json: &str, key: &str) -> Option<u32> {
    let rest = json_value(json, key)?;
    let end = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn subdirectories(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
                .map(|entry| entry.path())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ENTRY: &str = r#"{"pid":1509,"sessionId":"1ecf390b-551e-49be-b895-069bd6b0f6a5","cwd":"/Users/x/p","parentPid":7}"#;

    #[test]
    fn a_registry_entry_yields_its_pid_and_session() {
        assert_eq!(json_number(ENTRY, "pid"), Some(1509));
        assert_eq!(
            json_string(ENTRY, "sessionId"),
            Some("1ecf390b-551e-49be-b895-069bd6b0f6a5")
        );
        assert_eq!(json_number(r#"{"pid" : 42}"#, "pid"), Some(42));
    }

    #[test]
    fn only_claude_code_temp_folders_match() {
        assert!(is_temp_dir_name("claude-501"));
        assert!(is_temp_dir_name("claude-0"));
        assert!(!is_temp_dir_name("claude-"));
        assert!(!is_temp_dir_name("claude-peek"));
        assert!(!is_temp_dir_name("claude"));
    }

    #[test]
    fn a_running_or_unnamed_folder_may_be_running() {
        let live = "1ecf390b-551e-49be-b895-069bd6b0f6a5";
        let ended = Path::new("/t/-p/0a1368bf-d980-45f6-9293-1e5f06f47f14");
        let sessions = Sessions::Running(HashSet::from([live.to_owned()]));

        assert!(sessions.may_be_running(&Path::new("/t/-p").join(live)));
        assert!(!sessions.may_be_running(ended));
        // Truncated ids and folders an agent named itself say nothing.
        assert!(sessions.may_be_running(Path::new("/t/-p/1ecf390b")));
        assert!(sessions.may_be_running(Path::new("/t/agpscan")));
        assert!(Sessions::Unreadable.may_be_running(ended));
    }

    #[test]
    fn sessions_sit_under_project_folders() {
        let td = tempfile::tempdir().unwrap();
        let session = td
            .path()
            .join("-Users-x-p")
            .join("0a1368bf-d980-45f6-9293-1e5f06f47f14");
        let loose = td.path().join("agpscan");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::create_dir_all(&loose).unwrap();

        let mut entries = temp_entries(td.path());
        entries.sort();

        assert_eq!(entries, vec![session, loose]);
    }
}
