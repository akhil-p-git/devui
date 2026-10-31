//! Application state and key handling. Nothing here touches the terminal or runs a command, so it can
//! be tested directly.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::widgets::TableState;

use crate::gather::Project;
use crate::sys::SysInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Recent,
    Name,
    Dirty,
}

impl SortKey {
    pub fn label(self) -> &'static str {
        match self {
            SortKey::Recent => "recent",
            SortKey::Name => "name",
            SortKey::Dirty => "dirty",
        }
    }

    fn next(self) -> Self {
        match self {
            SortKey::Recent => SortKey::Name,
            SortKey::Name => SortKey::Dirty,
            SortKey::Dirty => SortKey::Recent,
        }
    }
}

/// What the event loop must do after a key press.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    Refresh,
    Lazygit(PathBuf),
}

pub struct App {
    pub root: PathBuf,
    pub projects: Vec<Project>,
    pub sys: Option<SysInfo>,
    pub table: TableState,
    pub sort: SortKey,
    pub message: Option<String>,
    pub loading: bool,
    /// Unix seconds used for "3d ago" labels; the event loop updates it before each draw.
    pub now: i64,
}

impl App {
    pub fn new(root: PathBuf) -> Self {
        App {
            root,
            projects: Vec::new(),
            sys: None,
            table: TableState::default(),
            sort: SortKey::Recent,
            message: None,
            loading: true,
            now: 0,
        }
    }

    pub fn selected(&self) -> Option<&Project> {
        self.table.selected().and_then(|i| self.projects.get(i))
    }

    /// Replaces the project list, keeping the same project selected when it still exists.
    pub fn set_projects(&mut self, projects: Vec<Project>) {
        let keep = self.selected().map(|p| p.path.clone());
        self.projects = projects;
        self.apply_sort();
        let index = keep
            .and_then(|path| self.projects.iter().position(|p| p.path == path))
            .or(if self.projects.is_empty() {
                None
            } else {
                Some(0)
            });
        self.table.select(index);
    }

    fn apply_sort(&mut self) {
        match self.sort {
            SortKey::Name => self.projects.sort_by_key(|p| p.name.to_lowercase()),
            SortKey::Recent => self
                .projects
                .sort_by_key(|p| std::cmp::Reverse(p.last_commit_time())),
            SortKey::Dirty => self.projects.sort_by(|a, b| {
                b.status
                    .changes
                    .len()
                    .cmp(&a.status.changes.len())
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            }),
        }
    }

    fn move_selection(&mut self, delta: isize) {
        if self.projects.is_empty() {
            return;
        }
        let last = self.projects.len() - 1;
        let current = self.table.selected().unwrap_or(0) as isize;
        self.table
            .select(Some((current + delta).clamp(0, last as isize) as usize));
    }

    fn cycle_sort(&mut self) {
        let keep = self.selected().map(|p| p.path.clone());
        self.sort = self.sort.next();
        self.apply_sort();
        let index = keep.and_then(|path| self.projects.iter().position(|p| p.path == path));
        self.table.select(index.or(if self.projects.is_empty() {
            None
        } else {
            Some(0)
        }));
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind != KeyEventKind::Press {
            return Action::None;
        }
        self.message = None;
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Action::Quit,
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_selection(1);
                Action::None
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_selection(-1);
                Action::None
            }
            KeyCode::PageDown => {
                self.move_selection(10);
                Action::None
            }
            KeyCode::PageUp => {
                self.move_selection(-10);
                Action::None
            }
            KeyCode::Home => {
                self.table.select(if self.projects.is_empty() {
                    None
                } else {
                    Some(0)
                });
                Action::None
            }
            KeyCode::End => {
                self.table.select(self.projects.len().checked_sub(1));
                Action::None
            }
            KeyCode::Char('s') => {
                self.cycle_sort();
                Action::None
            }
            KeyCode::Char('r') => Action::Refresh,
            KeyCode::Char('g') => match self.selected() {
                Some(p) => Action::Lazygit(p.path.clone()),
                None => Action::None,
            },
            _ => Action::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gather::{Change, Commit, Status};

    fn project(name: &str, dirty: usize, time: i64) -> Project {
        Project {
            name: name.to_string(),
            path: PathBuf::from(format!("/r/{name}")),
            status: Status {
                changes: (0..dirty)
                    .map(|i| Change {
                        code: " M".into(),
                        path: format!("f{i}"),
                    })
                    .collect(),
                ..Status::default()
            },
            recent: vec![Commit {
                hash: "abc".into(),
                time,
                subject: "s".into(),
            }],
            error: None,
        }
    }

    fn press(app: &mut App, c: KeyCode) -> Action {
        app.on_key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    fn app() -> App {
        let mut a = App::new(PathBuf::from("/r"));
        a.set_projects(vec![
            project("b", 0, 10),
            project("a", 3, 30),
            project("c", 1, 20),
        ]);
        a
    }

    #[test]
    fn starts_sorted_by_recent_with_first_selected() {
        let a = app();
        let names: Vec<_> = a.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "c", "b"]);
        assert_eq!(a.selected().unwrap().name, "a");
    }

    #[test]
    fn movement_clamps_at_both_ends() {
        let mut a = app();
        press(&mut a, KeyCode::Char('k'));
        assert_eq!(a.table.selected(), Some(0));
        for _ in 0..10 {
            press(&mut a, KeyCode::Char('j'));
        }
        assert_eq!(a.table.selected(), Some(2));
        press(&mut a, KeyCode::Home);
        assert_eq!(a.table.selected(), Some(0));
        press(&mut a, KeyCode::End);
        assert_eq!(a.table.selected(), Some(2));
    }

    #[test]
    fn sort_cycle_keeps_the_same_project_selected() {
        let mut a = app();
        press(&mut a, KeyCode::Char('j'));
        assert_eq!(a.selected().unwrap().name, "c");
        press(&mut a, KeyCode::Char('s'));
        assert_eq!(a.sort, SortKey::Name);
        assert_eq!(a.selected().unwrap().name, "c");
        press(&mut a, KeyCode::Char('s'));
        assert_eq!(a.sort, SortKey::Dirty);
        let names: Vec<_> = a.projects.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["a", "c", "b"]);
    }

    #[test]
    fn refresh_keeps_selection_by_path() {
        let mut a = app();
        press(&mut a, KeyCode::Char('j'));
        a.set_projects(vec![
            project("z", 0, 99),
            project("a", 3, 30),
            project("c", 1, 20),
        ]);
        assert_eq!(a.selected().unwrap().name, "c");
    }

    #[test]
    fn refresh_selects_first_when_selected_project_vanished() {
        let mut a = app();
        press(&mut a, KeyCode::End);
        a.set_projects(vec![project("z", 0, 99)]);
        assert_eq!(a.selected().unwrap().name, "z");
    }

    #[test]
    fn empty_list_has_no_selection_and_ignores_movement() {
        let mut a = App::new(PathBuf::from("/r"));
        a.set_projects(vec![]);
        press(&mut a, KeyCode::Char('j'));
        assert_eq!(a.selected().map(|p| p.name.clone()), None);
        assert_eq!(press(&mut a, KeyCode::Char('g')), Action::None);
    }

    #[test]
    fn keys_map_to_actions() {
        let mut a = app();
        assert_eq!(press(&mut a, KeyCode::Char('q')), Action::Quit);
        assert_eq!(press(&mut a, KeyCode::Char('r')), Action::Refresh);
        assert_eq!(
            press(&mut a, KeyCode::Char('g')),
            Action::Lazygit(PathBuf::from("/r/a"))
        );
        assert_eq!(
            a.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Action::Quit
        );
    }
}
