use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail};
use devui::app::{Action, App};
use devui::gather::{self, Project};
use devui::{sys, ui};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event};

const USAGE: &str = "devui [DIR]

A terminal dashboard for the git projects directly under DIR (default: ~/work/Dev) and the health of
this machine: disk use, a stale running kernel, and Docker's disk use. It only reads.

keys: j/k or arrows move, PageUp/PageDown/Home/End jump, s cycles the sort, r refreshes,
      g opens lazygit in the selected project, q quits";

const AUTO_REFRESH: Duration = Duration::from_secs(30);

enum Msg {
    Projects(Result<Vec<Project>, String>),
    Sys(sys::SysInfo),
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn on_path(program: &str) -> bool {
    env::var_os("PATH")
        .is_some_and(|paths| env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn spawn_refresh(root: &Path, home: &Path, tx: &Sender<Msg>) {
    let (root, tx_projects) = (root.to_path_buf(), tx.clone());
    thread::spawn(move || {
        let _ = tx_projects.send(Msg::Projects(
            gather::scan(&root).map_err(|e| format!("{e:#}")),
        ));
    });
    let (home, tx_sys) = (home.to_path_buf(), tx.clone());
    thread::spawn(move || {
        let _ = tx_sys.send(Msg::Sys(sys::collect(&home)));
    });
}

fn run(terminal: &mut DefaultTerminal, root: PathBuf, home: PathBuf) -> Result<()> {
    let (tx, rx): (Sender<Msg>, Receiver<Msg>) = mpsc::channel();
    let mut app = App::new(root.clone());
    let mut last_refresh = Instant::now();
    spawn_refresh(&root, &home, &tx);

    loop {
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Projects(Ok(projects)) => {
                    app.set_projects(projects);
                    app.loading = false;
                }
                Msg::Projects(Err(e)) => {
                    app.loading = false;
                    app.message = Some(e);
                }
                Msg::Sys(info) => app.sys = Some(info),
            }
        }
        app.now = now_secs();
        terminal.draw(|frame| ui::render(frame, &mut app))?;

        if event::poll(Duration::from_millis(200))?
            && let Event::Key(key) = event::read()?
        {
            match app.on_key(key) {
                Action::Quit => return Ok(()),
                Action::Refresh => {
                    if !app.loading {
                        app.loading = true;
                        last_refresh = Instant::now();
                        spawn_refresh(&root, &home, &tx);
                    }
                }
                Action::Lazygit(path) => {
                    if !on_path("lazygit") {
                        app.message =
                            Some("lazygit is not installed (sudo pacman -S lazygit)".to_string());
                        continue;
                    }
                    ratatui::restore();
                    let result = Command::new("lazygit").arg("-p").arg(&path).status();
                    *terminal = ratatui::init();
                    if let Err(e) = result {
                        app.message = Some(format!("could not run lazygit: {e}"));
                    }
                    app.loading = true;
                    last_refresh = Instant::now();
                    spawn_refresh(&root, &home, &tx);
                }
                Action::None => {}
            }
        }

        if !app.loading && last_refresh.elapsed() >= AUTO_REFRESH {
            app.loading = true;
            last_refresh = Instant::now();
            spawn_refresh(&root, &home, &tx);
        }
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        println!("{USAGE}");
        return Ok(());
    }
    if args.len() > 1 {
        bail!("expected at most one directory\n\n{USAGE}");
    }
    let Some(home) = env::var_os("HOME").map(PathBuf::from) else {
        bail!("HOME is not set");
    };
    let root = match args.first() {
        Some(dir) => PathBuf::from(dir),
        None => home.join("work/Dev"),
    };
    if !root.is_dir() {
        bail!("{} is not a directory", root.display());
    }

    let mut terminal = ratatui::init();
    let result = run(&mut terminal, root, home);
    ratatui::restore();
    result
}
