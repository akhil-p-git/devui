//! Rendering. Takes `&mut App` only because ratatui's table keeps its scroll offset in `TableState`.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Paragraph, Row, Table};

use crate::app::App;
use crate::gather::Project;
use crate::sys::{Disk, SysInfo, format_bytes};

const DIM: Style = Style::new().fg(Color::DarkGray);

/// "just now", "5m", "3h", "12d", "4mo", "2y".
pub fn format_age(now: i64, then: i64) -> String {
    let secs = (now - then).max(0);
    match secs {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", secs / 60),
        3600..=86_399 => format!("{}h", secs / 3600),
        86_400..=2_591_999 => format!("{}d", secs / 86_400),
        2_592_000..=31_535_999 => format!("{}mo", secs / 2_592_000),
        _ => format!("{}y", secs / 31_536_000),
    }
}

fn disk_color(percent: u64) -> Color {
    match percent {
        90.. => Color::Red,
        80..=89 => Color::Yellow,
        _ => Color::Green,
    }
}

fn disk_span(label: &str, disk: &Result<Disk, String>) -> Vec<Span<'static>> {
    match disk {
        Ok(d) => vec![
            Span::raw(format!("{label} ")),
            Span::styled(
                format!("{}% ", d.percent()),
                Style::new()
                    .fg(disk_color(d.percent()))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("({} free)", format_bytes(d.avail)), DIM),
        ],
        Err(e) => vec![
            Span::raw(format!("{label} ")),
            Span::styled(format!("unavailable: {e}"), Style::new().fg(Color::Red)),
        ],
    }
}

fn header_lines(sys: Option<&SysInfo>) -> Vec<Line<'static>> {
    let Some(sys) = sys else {
        return vec![Line::styled("reading system state...", DIM)];
    };

    let mut first = Vec::new();
    for (i, (label, disk)) in sys.disks.iter().enumerate() {
        if i > 0 {
            first.push(Span::styled("  |  ", DIM));
        }
        first.extend(disk_span(label, disk));
    }

    let mut second = Vec::new();
    match &sys.kernel {
        Ok(k) if k.reboot_needed() => second.push(Span::styled(
            format!(
                "kernel {} is stale (installed: {}): reboot before using Docker",
                k.running,
                k.installed.join(", ")
            ),
            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
        )),
        Ok(k) => second.push(Span::raw(format!("kernel {}", k.running))),
        Err(e) => second.push(Span::styled(
            format!("kernel unknown: {e}"),
            Style::new().fg(Color::Red),
        )),
    }

    let mut third = Vec::new();
    match &sys.docker {
        Ok(rows) if rows.is_empty() => third.push(Span::styled("docker: nothing stored", DIM)),
        Ok(rows) => {
            third.push(Span::styled("docker size/reclaimable: ", DIM));
            for (i, r) in rows.iter().enumerate() {
                if i > 0 {
                    third.push(Span::styled(" · ", DIM));
                }
                let reclaimable = r.reclaimable.split_whitespace().next().unwrap_or("?");
                let kind = r.kind.to_lowercase();
                let kind = kind.strip_prefix("local ").unwrap_or(&kind);
                third.push(Span::raw(format!("{kind} {}/{reclaimable}", r.size)));
            }
        }
        Err(e) => third.push(Span::styled(format!("docker: {e}"), DIM)),
    }
    vec![Line::from(first), Line::from(second), Line::from(third)]
}

fn dirty_style(project: &Project) -> Style {
    if project.error.is_some() {
        Style::new().fg(Color::Red)
    } else if project.status.changes.is_empty() {
        DIM
    } else {
        Style::new().fg(Color::Yellow)
    }
}

fn project_row(p: &Project, now: i64) -> Row<'static> {
    let st = &p.status;
    let branch = if p.error.is_some() {
        "error".to_string()
    } else {
        st.branch.clone()
    };
    let sync = match (&st.upstream, st.ahead, st.behind) {
        _ if p.error.is_some() => Cell::from(""),
        (None, ..) => Cell::from(Span::styled("no upstream", DIM)),
        (Some(_), 0, 0) => Cell::from(Span::styled("synced", DIM)),
        (Some(_), a, b) => {
            let mut spans = Vec::new();
            if a > 0 {
                spans.push(Span::styled(
                    format!("↑{a} "),
                    Style::new().fg(Color::Green),
                ));
            }
            if b > 0 {
                spans.push(Span::styled(format!("↓{b}"), Style::new().fg(Color::Red)));
            }
            Cell::from(Line::from(spans))
        }
    };
    let dirty = if st.changes.is_empty() {
        "-".to_string()
    } else {
        st.changes.len().to_string()
    };
    let age = p
        .last_commit_time()
        .map(|t| format_age(now, t))
        .unwrap_or_else(|| "-".to_string());
    Row::new(vec![
        Cell::from(p.name.clone()),
        Cell::from(branch),
        Cell::from(Span::styled(dirty, dirty_style(p))),
        sync,
        Cell::from(Span::styled(age, DIM)),
    ])
}

fn detail_lines(p: &Project, now: i64, height: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::styled(p.path.display().to_string(), DIM)];
    if let Some(e) = &p.error {
        lines.push(Line::styled(
            format!("git error: {e}"),
            Style::new().fg(Color::Red),
        ));
        return lines;
    }
    let st = &p.status;
    let upstream = st.upstream.as_deref().unwrap_or("no upstream");
    lines.push(Line::from(vec![
        Span::styled(st.branch.clone(), Style::new().add_modifier(Modifier::BOLD)),
        Span::styled(format!("  ->  {upstream}"), DIM),
    ]));
    lines.push(Line::raw(""));

    let commits_block = 2 + p.recent.len().max(1);
    let changes_budget = height
        .saturating_sub(lines.len() + 1 + commits_block)
        .max(2);
    if st.changes.is_empty() {
        lines.push(Line::styled("working tree clean", DIM));
    } else {
        lines.push(Line::styled(
            format!("Changes ({})", st.changes.len()),
            Style::new().add_modifier(Modifier::BOLD),
        ));
        let shown = if st.changes.len() > changes_budget {
            changes_budget - 1
        } else {
            st.changes.len()
        };
        for c in st.changes.iter().take(shown) {
            let color = if c.code == "??" {
                Color::Cyan
            } else {
                Color::Yellow
            };
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", c.code), Style::new().fg(color)),
                Span::raw(c.path.clone()),
            ]));
        }
        if shown < st.changes.len() {
            lines.push(Line::styled(
                format!("... {} more", st.changes.len() - shown),
                DIM,
            ));
        }
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        "Recent commits",
        Style::new().add_modifier(Modifier::BOLD),
    ));
    if p.recent.is_empty() {
        lines.push(Line::styled("no commits yet", DIM));
    }
    for c in &p.recent {
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", c.hash), Style::new().fg(Color::Magenta)),
            Span::styled(format!("{:>4}  ", format_age(now, c.time)), DIM),
            Span::raw(c.subject.clone()),
        ]));
    }
    lines
}

pub fn render(frame: &mut Frame, app: &mut App) {
    let [header, body, footer] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let title = format!(" devui · {} ", app.root.display());
    frame.render_widget(
        Paragraph::new(header_lines(app.sys.as_ref())).block(Block::bordered().title(title)),
        header,
    );

    let [list_area, detail_area] =
        Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)]).areas(body);
    render_list(frame, app, list_area);
    render_detail(frame, app, detail_area);

    let help = match &app.message {
        Some(m) => Line::styled(m.clone(), Style::new().fg(Color::Yellow)),
        None => {
            let busy = if app.loading { "refreshing...  " } else { "" };
            Line::from(vec![
                Span::styled(busy, Style::new().fg(Color::Cyan)),
                Span::styled(
                    format!(
                        "j/k move  s sort ({})  r refresh  g lazygit  q quit",
                        app.sort.label()
                    ),
                    DIM,
                ),
            ])
        }
    };
    frame.render_widget(Paragraph::new(help), footer);
}

fn render_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let now = app.now;
    let rows: Vec<Row> = app.projects.iter().map(|p| project_row(p, now)).collect();
    let header = Row::new(["Project", "Branch", "Dirty", "Sync", "Last"])
        .style(Style::new().add_modifier(Modifier::BOLD | Modifier::UNDERLINED));
    let table = Table::new(
        rows,
        [
            Constraint::Fill(3),
            Constraint::Fill(2),
            Constraint::Length(5),
            Constraint::Length(11),
            Constraint::Length(4),
        ],
    )
    .header(header)
    .block(Block::bordered().title(format!(" Projects ({}) ", app.projects.len())))
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED))
    .highlight_symbol("> ");
    frame.render_stateful_widget(table, area, &mut app.table);
}

fn render_detail(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::bordered().title(match app.selected() {
        Some(p) => format!(" {} ", p.name),
        None => " Details ".to_string(),
    });
    let inner_height = area.height.saturating_sub(2) as usize;
    let lines = match app.selected() {
        Some(p) => detail_lines(p, app.now, inner_height),
        None if app.loading => vec![Line::styled("scanning...", DIM)],
        None => vec![Line::styled(
            format!("no git projects under {}", app.root.display()),
            DIM,
        )],
    };
    frame.render_widget(Paragraph::new(lines).block(block), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gather::{Change, Commit, Status};
    use crate::sys::{DockerRow, Kernel};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn draw(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| render(f, app)).unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..h)
            .map(|y| {
                (0..w)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn sample_app() -> App {
        let mut app = App::new(PathBuf::from("/home/u/work/Dev"));
        app.now = 1_000_000;
        app.loading = false;
        app.set_projects(vec![
            Project {
                name: "aihub".into(),
                path: PathBuf::from("/x/aihub"),
                status: Status {
                    branch: "main".into(),
                    upstream: None,
                    changes: vec![Change {
                        code: " M".into(),
                        path: "README.md".into(),
                    }],
                    ..Status::default()
                },
                recent: vec![Commit {
                    hash: "abc1234".into(),
                    time: 1_000_000 - 7200,
                    subject: "docs: add install".into(),
                }],
                error: None,
            },
            Project {
                name: "spotui".into(),
                path: PathBuf::from("/x/spotui"),
                status: Status {
                    branch: "main".into(),
                    upstream: Some("origin/main".into()),
                    ahead: 2,
                    ..Status::default()
                },
                recent: vec![],
                error: None,
            },
        ]);
        app.sys = Some(SysInfo {
            disks: vec![
                (
                    "/".into(),
                    Ok(Disk {
                        size: 100,
                        used: 78,
                        avail: 22,
                    }),
                ),
                ("~/work".into(), Err("gone".into())),
            ],
            kernel: Ok(Kernel {
                running: "7.2.9-arch1-1".into(),
                installed: vec!["7.2.11-arch1-1".into()],
            }),
            docker: Ok(vec![DockerRow {
                kind: "Images".into(),
                size: "4GB".into(),
                reclaimable: "1GB".into(),
            }]),
        });
        app
    }

    #[test]
    fn ages_are_formatted_in_the_largest_unit() {
        assert_eq!(format_age(100, 100), "now");
        assert_eq!(format_age(1000, 700), "5m");
        assert_eq!(format_age(10_000, 0), "2h");
        assert_eq!(format_age(86_400 * 12, 0), "12d");
        assert_eq!(format_age(2_592_000 * 4, 0), "4mo");
        assert_eq!(format_age(31_536_000 * 2, 0), "2y");
        assert_eq!(format_age(5, 10), "now");
    }

    #[test]
    fn renders_projects_header_and_detail() {
        let mut app = sample_app();
        let screen = draw(&mut app, 120, 24);
        assert!(screen.contains("devui · /home/u/work/Dev"), "{screen}");
        assert!(screen.contains("78%"), "{screen}");
        assert!(screen.contains("~/work unavailable: gone"), "{screen}");
        assert!(
            screen.contains("is stale (installed: 7.2.11-arch1-1)"),
            "{screen}"
        );
        assert!(screen.contains("reboot before using Docker"), "{screen}");
        assert!(screen.contains("images 4GB/1GB"), "{screen}");
        assert!(screen.contains("aihub"), "{screen}");
        assert!(screen.contains("no upstream"), "{screen}");
        assert!(screen.contains("↑2"), "{screen}");
        assert!(screen.contains("Changes (1)"), "{screen}");
        assert!(screen.contains("docs: add install"), "{screen}");
    }

    #[test]
    fn empty_root_says_so() {
        let mut app = App::new(PathBuf::from("/nowhere"));
        app.loading = false;
        app.set_projects(vec![]);
        let screen = draw(&mut app, 100, 20);
        assert!(
            screen.contains("no git projects under /nowhere"),
            "{screen}"
        );
    }

    #[test]
    fn tiny_terminal_does_not_panic() {
        let mut app = sample_app();
        let _ = draw(&mut app, 20, 6);
        let _ = draw(&mut app, 1, 1);
    }

    #[test]
    fn long_change_lists_are_truncated_with_a_count() {
        let mut app = sample_app();
        app.projects[0].status.changes = (0..200)
            .map(|i| Change {
                code: "??".into(),
                path: format!("file{i}"),
            })
            .collect();
        app.table.select(Some(0));
        let screen = draw(&mut app, 120, 30);
        assert!(screen.contains("more"), "{screen}");
        assert!(screen.contains("Recent commits"), "{screen}");
    }
}
