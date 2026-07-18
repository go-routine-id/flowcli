//! flowcli — render flowmaid diagrams in the terminal.
//!
//!   flowcli diagram.mmd              # interactive
//!   flowcli diagram.mmd --preview    # print once (headless) and exit
//!   flowcli diagram.mmd --compact 200
//!
//! Interactive keys: q quit · Tab/BackTab focus a node · c toggle
//! compact (serpentine) layout · r reload the file.

mod render;

use flowmaid::model::Document;
use flowmaid::scene::Scene;
use render::Opts;
use std::time::Duration;

struct App {
    path: String,
    src: String,
    compact: Option<f64>,
    focus: Option<usize>, // index into the current scene's node ids
    scene: Scene,
    ids: Vec<String>,
    err: Option<String>,
}

impl App {
    fn rebuild(&mut self) {
        match build(&self.src, self.compact) {
            Ok(sc) => {
                self.ids = sc.nodes.iter().map(|n| n.id.clone()).collect();
                if let Some(f) = self.focus {
                    if f >= self.ids.len() {
                        self.focus = None;
                    }
                }
                self.scene = sc;
                self.err = None;
            }
            Err(e) => self.err = Some(e),
        }
    }
    fn opts(&self) -> Opts {
        Opts {
            highlight: self.focus.and_then(|i| self.ids.get(i)).cloned(),
            ..Opts::default()
        }
    }
}

/// Parse a source and pick the scene, honouring `--compact`. Only the
/// graph-shaped types carry the rich node/edge scene; ER and class ride
/// the same geometry, so they render too.
fn build(mmd: &str, compact: Option<f64>) -> Result<Scene, String> {
    match flowmaid::parser::parse_document(mmd).map_err(|e| e.to_string())? {
        Document::Flowchart(g) | Document::State(g) => Ok(match compact {
            Some(px) => {
                flowmaid::fold::scene_compact(&g, &flowmaid::fold::CompactOptions::for_extent(px))
                    .scene
            }
            None => flowmaid::scene::scene(&g),
        }),
        Document::Er(d) => Ok(flowmaid::er::scene(&d).scene),
        Document::Class(d) => Ok(flowmaid::class::scene(&d).scene),
        _ => Err("this diagram type isn't drawable in the terminal yet \
                  (flowchart, state, ER and class are)"
            .into()),
    }
}

fn main() {
    let mut path = None;
    let mut preview = false;
    let mut compact = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--preview" => preview = true,
            "--compact" => compact = args.next().and_then(|v| v.parse().ok()),
            "-h" | "--help" => {
                eprintln!("usage: flowcli <file.mmd> [--preview] [--compact <px>]");
                return;
            }
            other => path = Some(other.to_string()),
        }
    }
    let Some(path) = path else {
        eprintln!("usage: flowcli <file.mmd> [--preview] [--compact <px>]");
        std::process::exit(2);
    };
    let src = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot read {path}: {e}");
            std::process::exit(1);
        }
    };

    let mut app = App {
        path,
        src,
        compact,
        focus: None,
        scene: Scene::empty(0.0, 0.0),
        ids: Vec::new(),
        err: None,
    };
    app.rebuild();

    if preview {
        // Headless: print once at a fixed size and exit (verifiable).
        if let Some(e) = &app.err {
            eprintln!("parse error: {e}");
            std::process::exit(1);
        }
        print!("{}", render::preview(&app.scene, 110, 34, &app.opts()));
        return;
    }
    if let Err(e) = run(&mut app) {
        eprintln!("terminal error: {e}");
        std::process::exit(1);
    }
}

fn run(app: &mut App) -> std::io::Result<()> {
    use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
    use ratatui::layout::{Constraint, Layout};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};

    let mut term = ratatui::init();
    let result = loop {
        term.draw(|f| {
            let [top, body, help_row] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .areas(f.area());

            let focus = app.focus.and_then(|i| app.ids.get(i)).cloned();
            let title = Line::from(vec![
                Span::styled(" flowcli ", Style::default().fg(Color::Black).bg(Color::Cyan)),
                Span::raw(format!(" {} ", app.path)),
                Span::styled(
                    format!(
                        "· {} nodes · {} edges{}{} ",
                        app.scene.nodes.len(),
                        app.scene.edges.len(),
                        if app.compact.is_some() { " · compact" } else { "" },
                        focus.map(|f| format!(" · focus: {f}")).unwrap_or_default(),
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            f.render_widget(Paragraph::new(title), top);

            let canvas = Block::default().borders(Borders::ALL).border_style(
                Style::default().fg(Color::DarkGray),
            );
            let inner = canvas.inner(body);
            f.render_widget(canvas, body);
            if let Some(e) = &app.err {
                f.render_widget(
                    Paragraph::new(format!("parse error: {e}")).style(Style::default().fg(Color::Red)),
                    inner,
                );
            } else {
                render::draw(f.buffer_mut(), inner, &app.scene, &app.opts());
            }

            let keycap = Style::default().add_modifier(Modifier::REVERSED);
            let help = Paragraph::new(Line::from(vec![
                Span::styled(" q ", keycap),
                Span::raw(" quit  "),
                Span::styled(" Tab ", keycap),
                Span::raw(" focus node  "),
                Span::styled(" c ", keycap),
                Span::raw(" compact  "),
                Span::styled(" r ", keycap),
                Span::raw(" reload"),
            ]))
            .style(Style::default().fg(Color::DarkGray));
            f.render_widget(help, help_row);
        })?;

        if event::poll(Duration::from_millis(150))? {
            if let Event::Key(k) = event::read()? {
                if k.kind != KeyEventKind::Press {
                    continue;
                }
                match k.code {
                    KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                    KeyCode::Tab | KeyCode::Right | KeyCode::Down => {
                        app.focus = cycle(app.focus, app.ids.len(), 1);
                    }
                    KeyCode::BackTab | KeyCode::Left | KeyCode::Up => {
                        app.focus = cycle(app.focus, app.ids.len(), -1);
                    }
                    KeyCode::Char('c') => {
                        app.compact = if app.compact.is_some() {
                            None
                        } else {
                            Some(220.0)
                        };
                        app.rebuild();
                    }
                    KeyCode::Char('r') => {
                        if let Ok(s) = std::fs::read_to_string(&app.path) {
                            app.src = s;
                        }
                        app.rebuild();
                    }
                    _ => {}
                }
            }
        }
    };
    ratatui::restore();
    result
}

fn cycle(cur: Option<usize>, len: usize, dir: i64) -> Option<usize> {
    if len == 0 {
        return None;
    }
    match cur {
        None => Some(if dir > 0 { 0 } else { len - 1 }),
        Some(i) => {
            let n = (i as i64 + dir).rem_euclid(len as i64) as usize;
            Some(n)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_flowchart_to_box_and_line_drawing() {
        let sc = build("flowchart TD\nA[Start] --> B{OK?}\nB -->|yes| C[Go]\nB -->|no| A", None)
            .unwrap();
        let out = render::preview(&sc, 80, 24, &Opts::default());
        // Node ids all appear as labels, box corners and an arrow drawn.
        assert!(out.contains("Start") && out.contains("OK?") && out.contains("Go"));
        assert!(out.contains('╭') || out.contains('┌'), "node box corners");
        assert!(out.contains('─') && out.contains('│'), "line drawing");
        assert!(
            ['▶', '◀', '▲', '▼'].iter().any(|a| out.contains(*a)),
            "arrowheads"
        );
    }

    #[test]
    fn focus_dims_everything_but_the_chosen_node() {
        use ratatui::backend::TestBackend;
        use ratatui::style::Color;
        use ratatui::Terminal;
        let sc = build("flowchart LR\nA --> B --> C", None).unwrap();
        // Dimming is a COLOUR change (invisible in a plain-text dump),
        // so inspect the buffer: with B focused, every non-DarkGray
        // foreground belongs to B's cells; A and C go DarkGray.
        let mut term = Terminal::new(TestBackend::new(80, 20)).unwrap();
        term.draw(|f| {
            let area = f.area();
            render::draw(
                f.buffer_mut(),
                area,
                &sc,
                &Opts {
                    highlight: Some("B".into()),
                    ..Opts::default()
                },
            );
        })
        .unwrap();
        let buf = term.backend().buffer().clone();
        // The label 'B' cell keeps a bright colour; 'A' is dimmed.
        let find = |ch: char| {
            (0..20).flat_map(|y| (0..80).map(move |x| (x, y))).find(|&(x, y)| {
                buf[(x, y)].symbol() == ch.to_string()
            })
        };
        let (bx, by) = find('B').expect("B drawn");
        let (ax, ay) = find('A').expect("A drawn");
        assert_ne!(buf[(bx, by)].fg, Color::DarkGray, "focused node stays bright");
        assert_eq!(buf[(ax, ay)].fg, Color::DarkGray, "unfocused node dims");
    }

    #[test]
    fn compact_folds_a_long_chain_in_the_terminal() {
        let mut src = String::from("flowchart TD\n");
        for i in 0..13 {
            src.push_str(&format!("N{i} --> N{}\n", i + 1));
        }
        let tall = build(&src, None).unwrap();
        let folded = build(&src, Some(200.0)).unwrap();
        // Folding trades height for width, same node/edge count.
        assert_eq!(tall.nodes.len(), folded.nodes.len());
        assert!(folded.width > tall.width && folded.height < tall.height);
    }
}
