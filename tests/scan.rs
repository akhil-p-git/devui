//! Runs the real `git` against throwaway repositories.

use std::fs;
use std::path::Path;
use std::process::Command;

use devui::gather::scan;

fn git(dir: &Path, args: &[&str]) {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn init(dir: &Path) {
    fs::create_dir_all(dir).unwrap();
    git(dir, &["init", "-q", "-b", "main"]);
}

fn commit_file(dir: &Path, name: &str, msg: &str) {
    fs::write(dir.join(name), msg).unwrap();
    git(dir, &["add", name]);
    git(dir, &["commit", "-q", "-m", msg]);
}

#[test]
fn scan_reports_dirty_ahead_and_unborn_repos() {
    let root = tempfile::tempdir().unwrap();
    let r = root.path();

    // clean repo with a local upstream, one commit ahead, one dirty file, one untracked file
    let bare = r.join(".remotes/origin.git");
    fs::create_dir_all(&bare).unwrap();
    git(&bare, &["init", "-q", "--bare", "-b", "main"]);
    let alpha = r.join("alpha");
    init(&alpha);
    commit_file(&alpha, "a.txt", "first");
    git(&alpha, &["remote", "add", "origin", bare.to_str().unwrap()]);
    git(&alpha, &["push", "-q", "-u", "origin", "main"]);
    commit_file(&alpha, "b.txt", "second");
    fs::write(alpha.join("a.txt"), "changed").unwrap();
    fs::write(alpha.join("new file.txt"), "x").unwrap();

    // repo with no commits at all
    init(&r.join("empty"));

    // linked worktree: its `.git` is a file
    git(
        &alpha,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "side",
            r.join("wt").to_str().unwrap(),
        ],
    );

    // not a repo, and a hidden repo: both must be skipped
    fs::create_dir_all(r.join("plain")).unwrap();
    init(&r.join(".hidden"));

    let projects = scan(r).unwrap();
    let names: Vec<_> = projects.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["alpha", "empty", "wt"]);

    let alpha = &projects[0];
    assert_eq!(alpha.error, None);
    assert_eq!(alpha.status.branch, "main");
    assert_eq!(alpha.status.upstream.as_deref(), Some("origin/main"));
    assert_eq!((alpha.status.ahead, alpha.status.behind), (1, 0));
    let changes: Vec<_> = alpha
        .status
        .changes
        .iter()
        .map(|c| (c.code.as_str(), c.path.as_str()))
        .collect();
    assert_eq!(changes, [(" M", "a.txt"), ("??", "new file.txt")]);
    assert_eq!(alpha.recent.len(), 2);
    assert_eq!(alpha.recent[0].subject, "second");
    assert!(alpha.last_commit_time().is_some());

    let empty = &projects[1];
    assert_eq!(empty.error, None);
    assert!(empty.recent.is_empty());
    assert_eq!(empty.last_commit_time(), None);

    let wt = &projects[2];
    assert_eq!(wt.status.branch, "side");
    assert_eq!(wt.status.upstream, None);
}

#[test]
fn scan_of_a_missing_directory_is_an_error() {
    assert!(scan(Path::new("/definitely/not/here")).is_err());
}

#[test]
fn status_does_not_modify_the_repository() {
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("r");
    init(&repo);
    commit_file(&repo, "a.txt", "one");
    fs::write(repo.join("a.txt"), "dirty").unwrap();
    let before = fs::metadata(repo.join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(50));
    scan(root.path()).unwrap();
    let after = fs::metadata(repo.join(".git/index"))
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(before, after, "scan rewrote the git index");
}

#[cfg(unix)]
#[test]
fn scan_never_runs_a_command_named_in_the_repository_config() {
    use std::os::unix::fs::PermissionsExt;

    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("hostile");
    init(&repo);
    commit_file(&repo, "a.txt", "one");

    let marker = root.path().join("ran");
    let hook = root.path().join("hook.sh");
    fs::write(&hook, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    git(&repo, &["config", "core.fsmonitor", hook.to_str().unwrap()]);

    scan(root.path()).unwrap();
    assert!(
        !marker.exists(),
        "git ran core.fsmonitor from the scanned repository"
    );
}
