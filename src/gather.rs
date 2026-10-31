//! Reads the state of every git project under a root directory. Only read-only git commands run, and
//! `--no-optional-locks` keeps `git status` from taking the index lock another tool may be holding.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Two-character status such as ` M`, `A `, `??`.
    pub code: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Commit {
    pub hash: String,
    pub time: i64,
    pub subject: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    pub branch: String,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<Change>,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub name: String,
    pub path: PathBuf,
    pub status: Status,
    pub recent: Vec<Commit>,
    /// Set when `git status` failed, for example on a repository the current user may not read.
    pub error: Option<String>,
}

impl Project {
    pub fn last_commit_time(&self) -> Option<i64> {
        self.recent.first().map(|c| c.time)
    }
}

/// Parses `git status --porcelain=v2 --branch`.
pub fn parse_status(text: &str) -> Status {
    let mut st = Status::default();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("# branch.head ") {
            st.branch = rest.to_string();
        } else if let Some(rest) = line.strip_prefix("# branch.upstream ") {
            st.upstream = Some(rest.to_string());
        } else if let Some(rest) = line.strip_prefix("# branch.ab ") {
            let mut it = rest.split_whitespace();
            st.ahead = it
                .next()
                .and_then(|s| s.strip_prefix('+'))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            st.behind = it
                .next()
                .and_then(|s| s.strip_prefix('-'))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        } else if let Some(rest) = line.strip_prefix("1 ") {
            // 1 XY sub mH mI mW hH hI path
            push_entry(&mut st, rest, 7);
        } else if let Some(rest) = line.strip_prefix("2 ") {
            // 2 XY sub mH mI mW hH hI Xscore path<TAB>origPath
            push_entry(&mut st, rest, 8);
        } else if let Some(rest) = line.strip_prefix("u ") {
            // u XY sub m1 m2 m3 mW h1 h2 h3 path
            push_entry(&mut st, rest, 9);
        } else if let Some(path) = line.strip_prefix("? ") {
            st.changes.push(Change {
                code: "??".to_string(),
                path: path.to_string(),
            });
        }
    }
    st
}

/// `rest` is the entry without its leading type character, so it starts at the `XY` field.
/// `fields_before_path` counts the space-separated fields (XY included) that precede the path,
/// which may itself contain spaces.
fn push_entry(st: &mut Status, rest: &str, fields_before_path: usize) {
    let parts: Vec<&str> = rest.splitn(fields_before_path + 1, ' ').collect();
    if parts.len() <= fields_before_path {
        return;
    }
    // A rename or copy entry is "path<TAB>origPath".
    let path = parts[fields_before_path].split('\t').next().unwrap_or("");
    st.changes.push(Change {
        code: parts[0].replace('.', " "),
        path: path.to_string(),
    });
}

/// Parses `git log --format=%h<TAB>%ct<TAB>%s`.
pub fn parse_log(text: &str) -> Vec<Commit> {
    text.lines()
        .filter_map(|line| {
            let mut it = line.splitn(3, '\t');
            let hash = it.next()?.to_string();
            let time = it.next()?.parse().ok()?;
            let subject = it.next().unwrap_or("").to_string();
            Some(Commit {
                hash,
                time,
                subject,
            })
        })
        .collect()
}

fn git(path: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(path)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().next().unwrap_or("git failed").to_string())
    }
}

pub fn load_project(path: &Path) -> Project {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (status, error) = match git(path, &["status", "--porcelain=v2", "--branch"]) {
        Ok(text) => (parse_status(&text), None),
        Err(e) => (Status::default(), Some(e)),
    };
    // A repository with no commits makes `git log` fail; that is not an error worth showing.
    let recent = git(path, &["log", "-5", "--format=%h%x09%ct%x09%s"])
        .map(|t| parse_log(&t))
        .unwrap_or_default();
    Project {
        name,
        path: path.to_path_buf(),
        status,
        recent,
        error,
    }
}

/// Lists the non-hidden children of `root` that contain a `.git` entry (a directory, or a file for
/// a linked worktree), loads each one on its own thread, and returns them sorted by name.
pub fn scan(root: &Path) -> Result<Vec<Project>> {
    let mut repos = Vec::new();
    for entry in
        std::fs::read_dir(root).with_context(|| format!("cannot read {}", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        let hidden = entry.file_name().to_string_lossy().starts_with('.');
        if !hidden && path.is_dir() && path.join(".git").exists() {
            repos.push(path);
        }
    }
    let mut projects: Vec<Project> = thread::scope(|s| {
        let handles: Vec<_> = repos
            .iter()
            .map(|p| s.spawn(move || load_project(p)))
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    });
    projects.sort_by_key(|p| p.name.to_lowercase());
    Ok(projects)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# branch.oid 1234567890abcdef
# branch.head main
# branch.upstream origin/main
# branch.ab +2 -1
1 .M N... 100644 100644 100644 aaa bbb src/main.rs
1 A. N... 000000 100644 100644 000 ccc docs/with space.md
2 R. N... 100644 100644 100644 ddd eee R100 new name.rs\told.rs
u UU N... 100644 100644 100644 100644 f1 f2 f3 conflict.rs
? notes.txt
";

    #[test]
    fn parses_branch_and_counts() {
        let st = parse_status(SAMPLE);
        assert_eq!(st.branch, "main");
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (2, 1));
    }

    #[test]
    fn parses_every_entry_kind() {
        let st = parse_status(SAMPLE);
        let got: Vec<(&str, &str)> = st
            .changes
            .iter()
            .map(|c| (c.code.as_str(), c.path.as_str()))
            .collect();
        assert_eq!(
            got,
            vec![
                (" M", "src/main.rs"),
                ("A ", "docs/with space.md"),
                ("R ", "new name.rs"),
                ("UU", "conflict.rs"),
                ("??", "notes.txt"),
            ]
        );
    }

    #[test]
    fn branch_without_upstream_has_no_counts() {
        let st = parse_status("# branch.oid abc\n# branch.head feature\n");
        assert_eq!(st.upstream, None);
        assert_eq!((st.ahead, st.behind), (0, 0));
        assert!(st.changes.is_empty());
    }

    #[test]
    fn detached_head_keeps_git_label() {
        let st = parse_status("# branch.oid abc\n# branch.head (detached)\n");
        assert_eq!(st.branch, "(detached)");
    }

    #[test]
    fn parses_log_lines() {
        let log = parse_log("abc1234\t1700000000\tfix: a\tb\ndef5678\t1699990000\tfeat: c\n");
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].subject, "fix: a\tb");
        assert_eq!(log[1].time, 1699990000);
    }
}
