# devui

A terminal dashboard for the git projects directly under `~/work/Dev` and the health of the machine they live on. It only reads: every git call runs with `--no-optional-locks`, so it never takes an index lock another tool may hold.

```text
┌ devui · /home/akhil/work/Dev ─────────────────────────────────────────────┐
│/ 77% (10.7G free)  |  ~/work 23% (670.9G free)                            │
│kernel 7.2.11-arch1-1                                                      │
│docker size/reclaimable: images 5.361GB/28.7MB · volumes 1.041GB/845.3MB   │
└───────────────────────────────────────────────────────────────────────────┘
```

Below the header, the left pane lists every project with its branch, number of changed files, ahead/behind counts against its local upstream (no network fetch), and the age of its last commit. The right pane shows the selected project's changed files and last five commits.

The header goes red when a disk is at 90% or more, and shows a warning when the running kernel's modules are gone from `/usr/lib/modules`, which on Arch means a `pacman -Syu` upgraded `linux` and Docker networking breaks until you reboot.

## Install

Needs Linux, `git`, and Rust (built and tested with 1.96.0 on Arch). `df`, `uname`, and `timeout` come from coreutils; Docker is optional.

```bash
cd ~/work/Dev/devui
cargo build --release
ln -sfn "$PWD/target/release/devui" ~/.local/bin/devui   # ~/.local/bin must be on your PATH
```

## Use

```bash
devui            # scan ~/work/Dev
devui ~/other    # scan the git projects directly under another directory
```

| Key | Action |
| --- | --- |
| `j` / `k`, arrows | move |
| `PageUp` / `PageDown`, `Home` / `End` | jump |
| `s` | cycle the sort: recent commit, name, most changed files |
| `r` | refresh (it also refreshes every 30 seconds) |
| `g` | open `lazygit` in the selected project (must be installed) |
| `q`, `Esc` | quit |

A directory counts as a project when it has a `.git` entry (a directory, or a file for a linked worktree). Hidden directories are skipped.

## Develop

```bash
cargo fmt --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked
```

`tests/scan.rs` runs the real `git` against throwaway repositories, including one with an upstream, a linked worktree, and a repository with no commits.
