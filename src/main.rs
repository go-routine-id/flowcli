//! flowcli — render flowmaid diagrams in the terminal.
//!
//!   flowcli diagram.mmd              # interactive
//!   flowcli diagram.mmd --preview    # print once (headless) and exit
//!   flowcli diagram.mmd --compact 200
//!   flowcli --version | --self-update
//!
//! Interactive keys: q quit · Tab/BackTab focus a node · s save the
//! dragged layout to a `<file>.layout` sidecar (auto-loaded next open)
//! · c toggle compact (serpentine) layout · r reload the file.
//! Mouse: left-click picks whatever is under the cursor — node (focus
//! it), edge, or cluster — via the engine's `Scene::hit_test`; drag a
//! node to move it, edges re-route live via `route_partial` (flowchart
//! and state diagrams). Sequence diagrams are static: only q and r apply.

mod render;
mod update;

use flowmaid::class::ClassScene;
use flowmaid::er::ErScene;
use flowmaid::gitgraph::GitScene;
use flowmaid::model::{Document, Graph};
use flowmaid::scene::Scene;
use flowmaid::seq::SeqScene;
use render::Opts;
use std::time::Duration;

/// The parsed diagram. `Graph` diagrams (flowchart/state, non-compact)
/// keep their source graph so nodes can be dragged and re-routed live via
/// `route_partial`; everything else is a static `Plain` scene.
enum Built {
    Graph {
        g: Box<Graph>,
        scene: Scene,
    },
    Plain(Scene),
    GitGraph(GitScene),
    /// A sequence diagram: its own geometry, nothing to pick or drag.
    Sequence(SeqScene),
    /// A class diagram: its scene plus the members, end glyphs,
    /// cardinalities and notes the class overlay draws on top.
    Class(ClassScene),
    /// An ER diagram: its scene plus the tables' columns and the
    /// relationships' cardinalities the ER overlay draws on top.
    Er(ErScene),
}

/// Drag state for a graph diagram: the moving node plus its base routing,
/// so `route_partial` re-routes only the edges the drag touches.
struct GraphState {
    g: Graph,
    base: Scene,
    base_centers: Vec<(f64, f64)>,
    centers: Vec<(f64, f64)>, // current centres (mutated while dragging)
}

/// An in-progress node drag.
struct Drag {
    node: usize,
    /// Centre-minus-grab offset in scene px, so the node tracks the cursor
    /// from where it was actually grabbed instead of snapping its centre to it.
    off: (f64, f64),
}

struct App {
    path: String,
    src: String,
    compact: Option<f64>,
    focus: Option<usize>, // index into the current scene's node ids
    pick: Option<String>, // last click readout for an edge/cluster hit
    scene: Scene,
    ids: Vec<String>,
    graph: Option<GraphState>,   // present ⇒ draggable
    git_scene: Option<GitScene>, // present ⇒ gitGraph label overlay
    /// The finer scale a gitGraph ([`render::git_opts`]), class diagram
    /// ([`render::class_opts`]) or ER diagram ([`render::er_opts`]) is
    /// drawn at, computed once per rebuild.
    fit_opts: Option<Opts>,
    seq_scene: Option<SeqScene>, // present ⇒ drawn by the sequence painter
    class_scene: Option<ClassScene>, // present ⇒ class overlay
    er_scene: Option<ErScene>,   // present ⇒ ER overlay
    drag: Option<Drag>,
    err: Option<String>,
}

impl App {
    fn rebuild(&mut self) {
        match build(&self.src, self.compact) {
            Ok(built) => {
                let (scene, graph, git_scene, seq_scene, class_scene, er_scene) = match built {
                    Built::Graph { g, scene: fresh } => {
                        let ids: Vec<String> = fresh.nodes.iter().map(|n| n.id.clone()).collect();
                        let base_centers: Vec<(f64, f64)> =
                            fresh.nodes.iter().map(|n| (n.x, n.y)).collect();
                        // Apply a saved layout sidecar over the fresh
                        // auto-layout, then re-route the moved edges so
                        // the diagram opens where it was last dragged.
                        let mut centers = base_centers.clone();
                        let scene = if load_layout(&layout_path(&self.path), &ids, &mut centers) {
                            flowmaid::scene::route_partial(&g, &centers, &fresh, &base_centers)
                        } else {
                            fresh.clone()
                        };
                        let gs = GraphState {
                            g: *g,
                            base: fresh,
                            base_centers,
                            centers,
                        };
                        (scene, Some(gs), None, None, None, None)
                    }
                    Built::Plain(scene) => (scene, None, None, None, None, None),
                    Built::GitGraph(gs) => (gs.scene.clone(), None, Some(gs), None, None, None),
                    // The real scene, so classes and tables can be focused
                    // and picked.
                    Built::Class(cs) => (cs.scene.clone(), None, None, None, Some(cs), None),
                    Built::Er(es) => (es.scene.clone(), None, None, None, None, Some(es)),
                    // An empty scene of the same extent: picking and focus
                    // find nothing, the preview size rule still applies.
                    Built::Sequence(sq) => (
                        Scene::empty(sq.width, sq.height),
                        None,
                        None,
                        Some(sq),
                        None,
                        None,
                    ),
                };
                self.ids = scene.nodes.iter().map(|n| n.id.clone()).collect();
                if let Some(f) = self.focus {
                    if f >= self.ids.len() {
                        self.focus = None;
                    }
                }
                self.scene = scene;
                self.graph = graph;
                self.fit_opts = git_scene
                    .as_ref()
                    .map(|gs| render::git_opts(gs, &Opts::default()))
                    .or_else(|| {
                        class_scene
                            .as_ref()
                            .map(|cs| render::class_opts(cs, &Opts::default()))
                    })
                    .or_else(|| {
                        er_scene
                            .as_ref()
                            .map(|es| render::er_opts(es, &Opts::default()))
                    });
                self.git_scene = git_scene;
                self.seq_scene = seq_scene;
                self.class_scene = class_scene;
                self.er_scene = er_scene;
                self.drag = None;
                self.pick = None; // indices change on rebuild
                self.err = None;
            }
            Err(e) => self.err = Some(e),
        }
    }
    /// Draw/pick options. A gitGraph gets [`render::git_opts`], a class
    /// diagram [`render::class_opts`] and an ER diagram
    /// [`render::er_opts`], so draw, overlay and click picking all share
    /// one finer scale.
    fn opts(&self) -> Opts {
        Opts {
            highlight: self.focus.and_then(|i| self.ids.get(i)).cloned(),
            ..self.fit_opts.clone().unwrap_or_default()
        }
    }
    /// Write the current node positions to `<path>.layout` so a dragged
    /// arrangement survives across sessions. Mermaid has no syntax for
    /// absolute positions, so this is a sidecar (`<id> <x> <y>` per line)
    /// that `rebuild` reads back — the layout persists, the `.mmd` stays
    /// canonical. No-op for non-graph diagrams (nothing to drag).
    fn save_layout(&mut self) {
        let Some(gs) = &self.graph else {
            self.pick = Some("nothing to save — drag needs a flowchart/state diagram".into());
            return;
        };
        let mut out = String::new();
        for (i, id) in self.ids.iter().enumerate() {
            if let Some(&(x, y)) = gs.centers.get(i) {
                out.push_str(&format!("{id} {x} {y}\n"));
            }
        }
        let p = layout_path(&self.path);
        self.pick = Some(match std::fs::write(&p, out) {
            Ok(()) => format!("saved layout → {p}"),
            Err(e) => format!("save failed: {e}"),
        });
    }
    /// Move `node` so its centre sits at scene point `(x, y)` shifted by the
    /// grab `off`, re-routing only the edges the move touches. No-op for
    /// non-graph (ER/class/compact) diagrams, which carry no source graph.
    fn drag_node(&mut self, node: usize, x: f64, y: f64, off: (f64, f64)) {
        if let Some(gs) = &mut self.graph {
            if node < gs.centers.len() {
                gs.centers[node] = (x + off.0, y + off.1);
                self.scene =
                    flowmaid::scene::route_partial(&gs.g, &gs.centers, &gs.base, &gs.base_centers);
            }
        }
    }
}

/// The sidecar path for a diagram: `diagram.mmd` → `diagram.mmd.layout`.
fn layout_path(path: &str) -> String {
    format!("{path}.layout")
}

/// Read a `<id> <x> <y>` sidecar and overlay the saved positions onto
/// `centers` (by node id, so it survives node reordering). Returns
/// whether any position was applied; a missing or unreadable file is
/// simply "no saved layout" (`false`), never an error.
fn load_layout(path: &str, ids: &[String], centers: &mut [(f64, f64)]) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let mut applied = false;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        if let (Some(id), Some(xs), Some(ys)) = (it.next(), it.next(), it.next()) {
            if let (Ok(x), Ok(y), Some(i)) = (
                xs.parse::<f64>(),
                ys.parse::<f64>(),
                ids.iter().position(|n| n == id),
            ) {
                centers[i] = (x, y);
                applied = true;
            }
        }
    }
    applied
}

/// Parse a source and build a drawable scene, honouring `--compact`. Only
/// non-compact flowchart/state diagrams stay `Graph` (and so draggable);
/// compact folds and ER/class ride the same geometry but as static scenes.
fn build(mmd: &str, compact: Option<f64>) -> Result<Built, String> {
    match flowmaid::parser::parse_document(mmd).map_err(|e| e.to_string())? {
        Document::Flowchart(g) | Document::State(g) => Ok(match compact {
            Some(px) => Built::Plain(
                flowmaid::fold::scene_compact(&g, &flowmaid::fold::CompactOptions::for_extent(px))
                    .scene,
            ),
            None => {
                let scene = flowmaid::scene::scene(&g);
                Built::Graph {
                    g: Box::new(g),
                    scene,
                }
            }
        }),
        Document::Er(d) => Ok(Built::Er(flowmaid::er::scene(&d))),
        Document::Class(d) => Ok(Built::Class(flowmaid::class::scene(&d))),
        Document::GitGraph(d) => Ok(Built::GitGraph(flowmaid::gitgraph::scene(&d))),
        Document::Architecture(d) => Ok(Built::Plain(flowmaid::architecture::scene(&d).scene)),
        Document::Sequence(d) => Ok(Built::Sequence(flowmaid::seq::scene(&d))),
        _ => Err("this diagram type isn't drawable in the terminal yet \
                  (flowchart, state, ER, class, gitGraph, architecture, and sequence are)"
            .into()),
    }
}

const USAGE: &str =
    "usage: flowcli <file.mmd> [--preview] [--compact <px>]\n       flowcli --version | --self-update";

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
                eprintln!("{USAGE}");
                return;
            }
            "-V" | "--version" => {
                println!("flowcli {}", env!("CARGO_PKG_VERSION"));
                return;
            }
            "--self-update" => std::process::exit(update::self_update()),
            other => path = Some(other.to_string()),
        }
    }
    let Some(path) = path else {
        eprintln!("{USAGE}");
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
        pick: None,
        scene: Scene::empty(0.0, 0.0),
        ids: Vec::new(),
        graph: None,
        git_scene: None,
        fit_opts: None,
        seq_scene: None,
        class_scene: None,
        er_scene: None,
        drag: None,
        err: None,
    };
    app.rebuild();

    if preview {
        // Headless: print once at a fixed size and exit (verifiable).
        if let Some(e) = &app.err {
            eprintln!("parse error: {e}");
            std::process::exit(1);
        }
        let (cols, rows) = if app.scene.height > app.scene.width * 1.2 {
            (60, 50)
        } else {
            (110, 34)
        };
        let out = if let Some(sq) = &app.seq_scene {
            render::preview_seq(sq, cols, rows, &app.opts())
        } else if let Some(cs) = &app.class_scene {
            render::preview_class(cs, cols, rows, &app.opts())
        } else if let Some(es) = &app.er_scene {
            render::preview_er(es, cols, rows, &app.opts())
        } else if let Some(gs) = &app.git_scene {
            render::preview_with_git(gs, cols, rows, &app.opts())
        } else {
            render::preview(&app.scene, cols, rows, &app.opts())
        };
        print!("{out}");
        return;
    }
    if let Err(e) = run(&mut app) {
        eprintln!("terminal error: {e}");
        std::process::exit(1);
    }
}

fn run(app: &mut App) -> std::io::Result<()> {
    use flowmaid::scene::Hit;
    use ratatui::crossterm::event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEventKind,
    };
    use ratatui::crossterm::execute;
    use ratatui::layout::{Constraint, Layout, Rect};
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Borders, Paragraph};

    let mut term = ratatui::init();
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    // The inner canvas rect from the last frame, so a click can be mapped
    // back to scene pixels. Recomputed every draw (size may change).
    let mut canvas_area: Option<Rect> = None;
    let result = loop {
        term.draw(|f| {
            let [top, body, help_row] = Layout::vertical([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .areas(f.area());

            let focus = app.focus.and_then(|i| app.ids.get(i)).cloned();
            let counts = match &app.seq_scene {
                Some(sq) => format!(
                    "{} participants · {} messages",
                    sq.boxes.len(),
                    sq.messages.len()
                ),
                None => format!(
                    "{} nodes · {} edges",
                    app.scene.nodes.len(),
                    app.scene.edges.len()
                ),
            };
            let title = Line::from(vec![
                Span::styled(
                    " flowcli ",
                    Style::default().fg(Color::Black).bg(Color::Cyan),
                ),
                Span::raw(format!(" {} ", app.path)),
                Span::styled(
                    format!(
                        "· {counts}{}{}{} ",
                        if app.compact.is_some() && app.seq_scene.is_none() {
                            " · compact"
                        } else {
                            ""
                        },
                        focus.map(|f| format!(" · focus: {f}")).unwrap_or_default(),
                        app.pick
                            .as_deref()
                            .map(|p| format!(" · picked: {p}"))
                            .unwrap_or_default(),
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);
            f.render_widget(Paragraph::new(title), top);

            let canvas = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            let inner = canvas.inner(body);
            canvas_area = Some(inner);
            f.render_widget(canvas, body);
            if let Some(e) = &app.err {
                f.render_widget(
                    Paragraph::new(format!("parse error: {e}"))
                        .style(Style::default().fg(Color::Red)),
                    inner,
                );
            } else if let Some(sq) = &app.seq_scene {
                render::draw_seq(f.buffer_mut(), inner, sq, &app.opts());
            } else if let Some(cs) = &app.class_scene {
                render::draw_class(f.buffer_mut(), inner, cs, &app.opts());
            } else if let Some(es) = &app.er_scene {
                render::draw_er(f.buffer_mut(), inner, es, &app.opts());
            } else {
                render::draw(f.buffer_mut(), inner, &app.scene, &app.opts());
                if let Some(gs) = &app.git_scene {
                    render::draw_gitgraph_labels(f.buffer_mut(), inner, gs, &app.opts());
                }
            }

            let keycap = Style::default().add_modifier(Modifier::REVERSED);
            let keys = if app.seq_scene.is_some() {
                // Static diagram: nothing to focus, pick, drag or fold.
                vec![
                    Span::styled(" q ", keycap),
                    Span::raw(" quit  "),
                    Span::styled(" r ", keycap),
                    Span::raw(" reload"),
                ]
            } else {
                let mut keys = vec![
                    Span::styled(" q ", keycap),
                    Span::raw(" quit  "),
                    Span::styled(" Tab ", keycap),
                    Span::raw(" focus node  "),
                    Span::styled(" click ", keycap),
                    Span::raw(" pick  "),
                ];
                // Class boxes and ER tables are static: no drag, no layout
                // to save, and `build` ignores compact for them.
                if app.class_scene.is_none() && app.er_scene.is_none() {
                    keys.extend([
                        Span::styled(" drag ", keycap),
                        Span::raw(" move node  "),
                        Span::styled(" s ", keycap),
                        Span::raw(" save layout  "),
                        Span::styled(" c ", keycap),
                        Span::raw(" compact  "),
                    ]);
                }
                keys.extend([Span::styled(" r ", keycap), Span::raw(" reload")]);
                keys
            };
            let help = Paragraph::new(Line::from(keys)).style(Style::default().fg(Color::DarkGray));
            f.render_widget(help, help_row);
        })?;

        if event::poll(Duration::from_millis(150))? {
            match event::read()? {
                Event::Mouse(m) => {
                    let Some(area) = canvas_area else { continue };
                    match m.kind {
                        MouseEventKind::Down(MouseButton::Left) => {
                            match render::pick(&app.scene, area, m.column, m.row, &app.opts()) {
                                // A node pick drives the existing focus/dim
                                // highlight AND grabs the node for dragging
                                // (graph diagrams only — else it just focuses).
                                Some(Hit::Node(i)) => {
                                    app.focus = Some(i);
                                    app.pick = None;
                                    if app.graph.is_some() {
                                        let off = app
                                            .graph
                                            .as_ref()
                                            .and_then(|gs| gs.centers.get(i).copied())
                                            .zip(render::to_scene(
                                                &app.scene,
                                                area,
                                                m.column,
                                                m.row,
                                                &app.opts(),
                                            ))
                                            .map(|(c, p)| (c.0 - p.0, c.1 - p.1))
                                            .unwrap_or((0.0, 0.0));
                                        app.drag = Some(Drag { node: i, off });
                                    }
                                }
                                Some(Hit::Edge(i)) => {
                                    app.pick = app
                                        .scene
                                        .edges
                                        .get(i)
                                        .map(|e| format!("edge {}→{}", e.from, e.to));
                                }
                                Some(Hit::Cluster(i)) => {
                                    app.pick = app.scene.clusters.get(i).map(|c| {
                                        let name = if c.title.is_empty() {
                                            c.id.as_str()
                                        } else {
                                            c.title.as_str()
                                        };
                                        format!("cluster {name}")
                                    });
                                }
                                // Click on empty space clears the selection.
                                None => {
                                    app.focus = None;
                                    app.pick = None;
                                }
                            }
                        }
                        // Live drag: move the grabbed node under the cursor
                        // and re-route only its edges.
                        MouseEventKind::Drag(MouseButton::Left) => {
                            if let Some(d) = app.drag.as_ref().map(|d| (d.node, d.off)) {
                                if let Some((x, y)) =
                                    render::to_scene(&app.scene, area, m.column, m.row, &app.opts())
                                {
                                    app.drag_node(d.0, x, y, d.1);
                                }
                            }
                        }
                        MouseEventKind::Up(MouseButton::Left) => app.drag = None,
                        _ => {}
                    }
                }
                Event::Key(k) => {
                    if k.kind != KeyEventKind::Press {
                        continue;
                    }
                    match k.code {
                        KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                        KeyCode::Tab | KeyCode::Right | KeyCode::Down => {
                            app.focus = cycle(app.focus, app.ids.len(), 1);
                            app.pick = None;
                        }
                        KeyCode::BackTab | KeyCode::Left | KeyCode::Up => {
                            app.focus = cycle(app.focus, app.ids.len(), -1);
                            app.pick = None;
                        }
                        KeyCode::Char('c') => {
                            app.compact = if app.compact.is_some() {
                                None
                            } else {
                                Some(220.0)
                            };
                            app.rebuild();
                        }
                        KeyCode::Char('s') => app.save_layout(),
                        KeyCode::Char('r') => {
                            if let Ok(s) = std::fs::read_to_string(&app.path) {
                                app.src = s;
                            }
                            app.rebuild();
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    };
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
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
    fn layout_sidecar_round_trips_dragged_positions() {
        // load_layout overlays saved positions by node id, ignoring
        // unknown ids and tolerating a missing file.
        let ids = vec!["A".to_string(), "B".to_string()];
        let mut centers = vec![(0.0, 0.0), (0.0, 0.0)];
        // A saved sidecar written in the exact `save_layout` format.
        let sidecar = format!("{} {} {}\n{} {} {}\n", "A", 12.5, 40.0, "B", 300.0, 180.0);
        let dir = std::env::temp_dir();
        let path = dir.join("flowcli_test_layout.mmd.layout");
        std::fs::write(&path, &sidecar).unwrap();
        let p = path.to_str().unwrap();
        assert!(load_layout(p, &ids, &mut centers), "positions applied");
        assert_eq!(centers[0], (12.5, 40.0));
        assert_eq!(centers[1], (300.0, 180.0));
        // An unknown id is skipped; a matching one still applies.
        let mut c2 = vec![(1.0, 1.0)];
        std::fs::write(&path, "Z 9 9\nA 7 8\n").unwrap();
        assert!(load_layout(p, &["A".into()], &mut c2));
        assert_eq!(c2[0], (7.0, 8.0));
        // Missing file → no-op, no error.
        std::fs::remove_file(&path).ok();
        assert!(!load_layout(p, &ids, &mut centers));
        // The write half produces text load_layout reads back verbatim.
        assert_eq!(layout_path("d.mmd"), "d.mmd.layout");
    }

    /// Just the scene, regardless of whether it stayed a draggable graph.
    fn scene_of(mmd: &str, compact: Option<f64>) -> Scene {
        match build(mmd, compact).unwrap() {
            Built::Graph { scene, .. } | Built::Plain(scene) => scene,
            Built::GitGraph(gs) => gs.scene,
            Built::Sequence(sq) => Scene::empty(sq.width, sq.height),
            Built::Class(cs) => cs.scene,
            Built::Er(es) => es.scene,
        }
    }

    #[test]
    fn renders_a_flowchart_to_box_and_line_drawing() {
        let sc = scene_of(
            "flowchart TD\nA[Start] --> B{OK?}\nB -->|yes| C[Go]\nB -->|no| A",
            None,
        );
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
        let sc = scene_of("flowchart LR\nA --> B --> C", None);
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
            (0..20)
                .flat_map(|y| (0..80).map(move |x| (x, y)))
                .find(|&(x, y)| buf[(x, y)].symbol() == ch.to_string())
        };
        let (bx, by) = find('B').expect("B drawn");
        let (ax, ay) = find('A').expect("A drawn");
        assert_ne!(
            buf[(bx, by)].fg,
            Color::DarkGray,
            "focused node stays bright"
        );
        assert_eq!(buf[(ax, ay)].fg, Color::DarkGray, "unfocused node dims");
    }

    #[test]
    fn click_picks_the_node_under_the_cursor() {
        use flowmaid::scene::Hit;
        use ratatui::layout::Rect;
        let sc = scene_of("flowchart LR\nA[Alpha] --> B[Beta] --> C[Gamma]", None);
        let area = Rect::new(0, 0, 80, 24);
        let opts = Opts::default();
        // Aim at each node's centre cell via the SAME transform draw uses;
        // hit_test must return that node — proving screen→scene picking
        // lines up with what was drawn.
        let (sx, sy) = render::scale(&sc, area.width, area.height, &opts);
        for (i, n) in sc.nodes.iter().enumerate() {
            let col = area.x + (n.x / sx).round() as u16;
            let row = area.y + (n.y / sy).round() as u16;
            assert_eq!(
                render::pick(&sc, area, col, row, &opts),
                Some(Hit::Node(i)),
                "node {} at cell ({col},{row})",
                n.id
            );
        }
        // A far corner is empty space → no pick, and focus clears.
        assert_eq!(render::pick(&sc, area, 79, 23, &opts), None);
    }

    #[test]
    fn compact_folds_a_long_chain_in_the_terminal() {
        let mut src = String::from("flowchart TD\n");
        for i in 0..13 {
            src.push_str(&format!("N{i} --> N{}\n", i + 1));
        }
        let tall = scene_of(&src, None);
        let folded = scene_of(&src, Some(200.0));
        // Folding trades height for width, same node/edge count.
        assert_eq!(tall.nodes.len(), folded.nodes.len());
        assert!(folded.width > tall.width && folded.height < tall.height);
    }

    #[test]
    fn dragging_a_node_reroutes_only_its_own_edges() {
        // A graph diagram keeps its source graph, so a drag can re-route
        // via route_partial — the second half of pick → drag → route.
        let Built::Graph { g, scene } = build("flowchart TD\nA --> B --> C", None).unwrap() else {
            panic!("flowchart should stay a draggable graph");
        };
        let base_centers: Vec<(f64, f64)> = scene.nodes.iter().map(|n| (n.x, n.y)).collect();
        let mut centers = base_centers.clone();
        // Shove B sideways by 200px and re-route.
        centers[1].0 += 200.0;
        let moved = flowmaid::scene::route_partial(&g, &centers, &scene, &base_centers);
        assert_eq!(moved.nodes.len(), scene.nodes.len());
        // B moved ~200px relative to A (compare deltas so any global
        // canvas shift inside route_partial cancels out)...
        let base_rel = scene.nodes[1].x - scene.nodes[0].x;
        let moved_rel = moved.nodes[1].x - moved.nodes[0].x;
        assert!(
            (moved_rel - base_rel - 200.0).abs() < 1.0,
            "B should track the drag"
        );
        // ...while A and C keep their relative position (untouched by the move).
        let base_ac = scene.nodes[2].x - scene.nodes[0].x;
        let moved_ac = moved.nodes[2].x - moved.nodes[0].x;
        assert!((moved_ac - base_ac).abs() < 1.0, "unmoved nodes stay put");
    }

    #[test]
    fn er_diagrams_are_not_draggable() {
        // ER rides different geometry with no source graph → no drag.
        assert!(matches!(
            build("erDiagram\n a ||--o{ b : has", None).unwrap(),
            Built::Er(_)
        ));
    }

    #[test]
    fn gitgraph_renders_branch_labels_and_tags() {
        let Built::GitGraph(gs) = build(
            "gitGraph\n\
             commit id: \"init\"\n\
             branch feat\n\
             checkout feat\n\
             commit id: \"a\" tag: \"v1.0\"\n\
             checkout main\n\
             merge feat id: \"m\"",
            None,
        )
        .unwrap() else {
            panic!("gitGraph should become a GitGraph built");
        };
        let out = render::preview_with_git(&gs, 80, 20, &render::git_opts(&gs, &Opts::default()));
        assert!(out.contains('●'), "commit marker drawn");
        assert!(out.contains("main"), "branch label drawn");
        assert!(out.contains("feat"), "branch label drawn");
        assert!(out.contains("v1.0"), "tag drawn");
        assert!(out.contains("init"), "commit id drawn");
    }

    #[test]
    fn gitgraph_commit_kinds_get_their_own_glyph_on_the_lane() {
        // flowmaid ≥ 0.33 tells commit kinds apart by node shape: merge =
        // DoubleCircle, HIGHLIGHT = Rect, REVERSE = Circle + REVERSE_MARK.
        let Built::GitGraph(gs) = build(include_str!("../examples/git.mmd"), None).unwrap() else {
            panic!("gitGraph should become a GitGraph built");
        };
        let out = render::preview_with_git(&gs, 110, 34, &render::git_opts(&gs, &Opts::default()));
        assert!(out.contains('◉'), "merge commit glyph");
        assert!(out.contains('■'), "HIGHLIGHT commit glyph");
        assert!(out.contains('✕'), "REVERSE commit glyph");
        // No commit is drawn as a box any more (edge elbows use the
        // rounded glyphs, so square corners can only come from a box).
        assert!(
            !['┌', '┐', '└', '┘'].iter().any(|c| out.contains(*c)),
            "no commit boxes in:\n{out}"
        );
        // Each glyph sits ON its lane: the row with the `main` branch
        // label also carries main's commits, not the row below it.
        let main_row = out.lines().find(|l| l.contains("main")).expect("main lane");
        assert!(
            main_row.contains('●') && main_row.contains('◉'),
            "main commits on the lane row: {main_row:?}"
        );
    }

    #[test]
    fn gitgraph_commit_ids_do_not_run_together() {
        // The engine packs columns by its own text measure; the terminal
        // scale must follow it or neighbouring ids overwrite each other.
        let Built::GitGraph(gs) = build(include_str!("../examples/git.mmd"), None).unwrap() else {
            panic!("gitGraph should become a GitGraph built");
        };
        let out = render::preview_with_git(&gs, 110, 34, &render::git_opts(&gs, &Opts::default()));
        for id in ["init", "merge-feat", "merge-hotfix", "release", "fix"] {
            assert!(out.contains(id), "commit id {id:?} intact in:\n{out}");
        }
        assert!(
            render::git_opts(&gs, &Opts::default()).sx < Opts::default().sx,
            "gitGraph needs a finer scale"
        );
    }

    #[test]
    fn gitgraph_too_wide_elides_ids_instead_of_overlapping() {
        // Too wide for the canvas: scale() squeezes it to fit, leaving the
        // ids no room. Each drawn id is whole or a `…`-cut prefix, never
        // run into its neighbour; the focused commit's id stays whole.
        let mut src = String::from("gitGraph\n");
        let ids: Vec<String> = (1..=25).map(|i| format!("commit-{i}")).collect();
        for id in &ids {
            src.push_str(&format!("  commit id: \"{id}\"\n"));
        }
        let Built::GitGraph(gs) = build(&src, None).unwrap() else {
            panic!("gitGraph should become a GitGraph built");
        };
        let base = render::git_opts(&gs, &Opts::default());
        for focus in [None, Some("commit-2")] {
            let opts = Opts {
                highlight: focus.map(String::from),
                ..base.clone()
            };
            let out = render::preview_with_git(&gs, 110, 34, &opts);
            let row = out.lines().find(|l| l.contains("commit-")).expect("id row");
            for tok in row.split_whitespace() {
                let ok = match tok.strip_suffix('…') {
                    Some(cut) => ids.iter().any(|id| id.starts_with(cut) && id != cut),
                    None => ids.iter().any(|id| id == tok),
                };
                assert!(ok, "{tok:?} is neither an id nor a cut one in:\n{out}");
            }
            if let Some(f) = focus {
                assert!(
                    row.split_whitespace().any(|t| t == f),
                    "focused id {f:?} whole in:\n{out}"
                );
            }
        }
    }

    #[test]
    fn gitgraph_tb_keeps_ids_on_canvas_and_tags_off_legs() {
        // TB puts end-anchored ids left of the leftmost lane, and nudges a
        // tag off a merge leg by less than one cell's height.
        let src = include_str!("../examples/git.mmd").replacen("gitGraph", "gitGraph TB:", 1);
        let Built::GitGraph(gs) = build(&src, None).unwrap() else {
            panic!("gitGraph should become a GitGraph built");
        };
        let out = render::preview_with_git(&gs, 60, 50, &render::git_opts(&gs, &Opts::default()));
        assert!(
            out.lines().any(|l| l.contains("merge-hotfix")),
            "leftmost id clipped in:\n{out}"
        );
        let tag_row = out.lines().find(|l| l.contains("v1.0")).expect("tag drawn");
        assert!(
            !tag_row.contains('─'),
            "tag drawn over a leg: {tag_row:?}\n{out}"
        );
    }

    /// A `◀`/`▶` sitting inside a horizontal box border (`┌▶──`, `╭──◀─╮`):
    /// an edge entering a top/bottom face drawn pointing sideways.
    fn sideways_heads_on_borders(out: &str) -> Vec<String> {
        const BORDER: &str = "─┌┐└┘╭╮╰╯";
        out.lines()
            .filter(|l| {
                let cs: Vec<char> = l.chars().collect();
                cs.windows(3)
                    .any(|w| "◀▶".contains(w[1]) && BORDER.contains(w[0]) && BORDER.contains(w[2]))
            })
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn arrowheads_enter_top_and_bottom_faces_square() {
        // State boxes are wide and short, so an edge entering one off-centre
        // used to point at the box centre — sideways — instead of into the
        // face it touches (issue #6). Retry enters Idle from below.
        for src in [
            include_str!("../examples/state.mmd"),
            include_str!("../examples.mmd"),
        ] {
            let Built::Graph { scene, .. } = build(src, None).unwrap() else {
                panic!("should stay a draggable graph");
            };
            for (cols, rows) in [(110, 34), (60, 50)] {
                let out = render::preview(&scene, cols, rows, &Opts::default());
                let bad = sideways_heads_on_borders(&out);
                assert!(bad.is_empty(), "sideways heads on {bad:?}\n{out}");
            }
        }
        let Built::Graph { scene, .. } =
            build(include_str!("../examples/state.mmd"), None).unwrap()
        else {
            unreachable!()
        };
        let out = render::preview(&scene, 60, 50, &Opts::default());
        assert!(
            out.lines().any(|l| l.contains('▲') && l.contains('╰')),
            "retry should enter Idle's bottom face as ▲:\n{out}"
        );
    }

    /// The sequence scene of a source, failing the test for any other type.
    fn seq_of(mmd: &str) -> SeqScene {
        match build(mmd, None).unwrap() {
            Built::Sequence(sq) => sq,
            _ => panic!("not built as a sequence diagram"),
        }
    }

    #[test]
    fn sequence_example_draws_participants_top_and_bottom() {
        let sq = seq_of(include_str!("../examples/sequence.mmd"));
        let out = render::preview_seq(&sq, 110, 34, &Opts::default());
        for name in ["User", "Web", "API", "DB"] {
            assert!(
                out.matches(name).count() >= 2,
                "{name} in header and footer:\n{out}"
            );
        }
        assert!(out.contains('┆'), "dashed lifelines:\n{out}");
        assert!(out.contains('┃'), "activation bars:\n{out}");
        assert!(out.contains("refresh the token"), "note text:\n{out}");
    }

    #[test]
    fn sequence_draws_one_head_per_arrow_type() {
        const HEADS: [char; 5] = ['▶', '◀', '▷', '◁', '✕'];
        for (op, want) in [
            ("->>", Some('▶')),
            ("-->>", Some('▶')),
            ("-)", Some('▷')),
            ("-x", Some('✕')),
            ("->", None),
        ] {
            let sq = seq_of(&format!("sequenceDiagram\nA{op}B: hi"));
            let out = render::preview_seq(&sq, 60, 12, &Opts::default());
            let found: Vec<char> = out.chars().filter(|c| HEADS.contains(c)).collect();
            assert_eq!(found, want.into_iter().collect::<Vec<_>>(), "{op}:\n{out}");
            let dashed = op.starts_with("--");
            assert_eq!(out.contains('╌'), dashed, "{op} line style:\n{out}");
        }
        // Right-to-left messages point the other way.
        let sq = seq_of("sequenceDiagram\nparticipant A\nparticipant B\nB->>A: back");
        let out = render::preview_seq(&sq, 60, 12, &Opts::default());
        assert!(out.contains('◀') && !out.contains('▶'), "{out}");
    }

    #[test]
    fn sequence_draws_frames_and_autonumber() {
        let sq = seq_of(include_str!("../examples/sequence.mmd"));
        let out = render::preview_seq(&sq, 110, 34, &Opts::default());
        for chip in [
            "[alt] token refreshed",
            "[else] refresh failed",
            "[loop] every 30s",
        ] {
            assert!(out.contains(chip), "{chip}:\n{out}");
        }
        assert!(out.contains("1. open dashboard"), "{out}");
        assert!(out.contains("10. dashboard"), "{out}");
        // A self-message loops back into its own lifeline.
        assert!(out.contains('╮') && out.contains('╯'), "{out}");
    }

    #[test]
    fn sequence_survives_tiny_areas() {
        let sq = seq_of(include_str!("../examples/sequence.mmd"));
        for (c, r) in [(1, 1), (2, 1), (5, 3), (12, 6), (30, 10)] {
            let out = render::preview_seq(&sq, c, r, &Opts::default());
            assert_eq!(out.lines().count(), r as usize);
        }
    }

    /// The class scene of a source, failing the test for any other type.
    fn class_of(mmd: &str) -> ClassScene {
        match build(mmd, None).unwrap() {
            Built::Class(cs) => cs,
            _ => panic!("not built as a class diagram"),
        }
    }

    /// The class preview at the app's own scale.
    fn class_preview(cs: &ClassScene, cols: u16, rows: u16) -> String {
        render::preview_class(cs, cols, rows, &render::class_opts(cs, &Opts::default()))
    }

    #[test]
    fn class_example_draws_members_glyphs_cards_labels_and_note() {
        let cs = class_of(include_str!("../examples/class.mmd"));
        let out = class_preview(&cs, 110, 34);
        for text in [
            "«interface»",
            "+uuid id",
            "+Money total",
            "+place()",
            "+string email",
            "+pay(Money amount) : bool",
            "+send(string to)",
            "places",
            "contains",
            "notifies",
            "Total is cached",
            "after place()",
        ] {
            assert!(out.contains(text), "{text}:\n{out}");
        }
        // Compartment dividers, inheritance/realization triangles,
        // composition and aggregation diamonds, a dependency arrow,
        // dashed lines and the cardinalities.
        for glyph in ['├', '△', '◆', '◇', '▼', '╎'] {
            assert!(out.contains(glyph), "{glyph}:\n{out}");
        }
        assert!(out.contains(" 1 ") && out.contains(" * "), "{out}");
    }

    #[test]
    fn class_draws_one_glyph_per_relation_kind() {
        for (rel, glyph) in [
            ("<|--", '△'),
            ("<|..", '△'),
            ("*--", '◆'),
            ("o--", '◇'),
            ("<--", '▲'),
            ("<..", '▲'),
        ] {
            let cs = class_of(&format!("classDiagram\n  A {rel} B"));
            let out = class_preview(&cs, 60, 30);
            assert_eq!(out.matches(glyph).count(), 1, "{rel}:\n{out}");
        }
        // A plain link has no glyph, just junctions where it meets the boxes.
        let cs = class_of("classDiagram\n  A -- B");
        let out = class_preview(&cs, 60, 30);
        assert!(!out.contains(['△', '▽', '◆', '◇', '▲', '▼']), "{out}");
        assert!(out.contains('┬') && out.contains('┴'), "{out}");
        // Two-way: a glyph at both ends.
        let cs = class_of("classDiagram\n  A <|--|> B");
        let out = class_preview(&cs, 60, 30);
        assert!(out.contains('△') && out.contains('▽'), "{out}");
    }

    #[test]
    fn class_dashes_realization_and_dependency_only() {
        let dashed = |src: &str| class_preview(&class_of(src), 60, 30).contains('╎');
        assert!(dashed("classDiagram\n  A <|.. B"));
        assert!(dashed("classDiagram\n  A <.. B"));
        assert!(!dashed("classDiagram\n  A <|-- B"));
        assert!(!dashed("classDiagram\n  A <-- B"));
    }

    #[test]
    fn class_dashed_plain_link_has_no_arrowhead() {
        // `..` is dashed and undirected: no head, unlike `..>`.
        let cs = class_of("classDiagram\n  A .. B : uses");
        let out = class_preview(&cs, 60, 30);
        assert!(!out.contains(['▲', '▼', '▶', '◀']), "{out}");
        assert!(out.contains('╎') && out.contains("uses"), "{out}");
        let cs = class_of("classDiagram\n  A ..> B");
        assert_eq!(class_preview(&cs, 60, 30).matches('▼').count(), 1);
    }

    #[test]
    fn class_generic_names_keep_their_parameter() {
        // `I`/`B` type parameters are not `<i>`/`<b>` tags.
        let cs = class_of("classDiagram\n  class Box~I~ {\n    +int x\n  }\n  class Repo~B~");
        let out = class_preview(&cs, 60, 20);
        assert!(out.contains("Box<I>") && out.contains("Repo<B>"), "{out}");
    }

    #[test]
    fn class_wide_characters_fit_their_box() {
        let cs = class_of("classDiagram\n  class A {\n    +String 名前データ\n    +int ok\n  }");
        let out = class_preview(&cs, 60, 20);
        assert!(out.contains("+String 名前データ"), "{out}");
        // Every row keeps the grid's width, so the right border lines up.
        let width = |l: &str| ratatui::text::Span::raw(l).width();
        assert!(out.lines().all(|l| width(l) == 60), "{out}");
        let right = |l: &str| width(&l[..l.rfind('│').unwrap()]);
        let rows: Vec<_> = out.lines().filter(|l| l.contains('│')).map(right).collect();
        assert!(rows.windows(2).all(|w| w[0] == w[1]), "{rows:?}\n{out}");
    }

    #[test]
    fn class_lollipop_name_fits_its_spot() {
        let cs = class_of("classDiagram\n  VeryLongInterfaceName ()-- foo\n  class bar");
        let out = class_preview(&cs, 80, 20);
        assert!(out.contains("VeryLongInterfaceName"), "{out}");
        // A cramped area cuts the name instead of spilling it.
        let opts = Opts {
            sx: 40.0,
            ..render::class_opts(&cs, &Opts::default())
        };
        let out = render::preview_class(&cs, 20, 10, &opts);
        assert!(
            out.lines()
                .all(|l| ratatui::text::Span::raw(l).width() == 20),
            "{out}"
        );
    }

    #[test]
    fn class_scale_is_never_coarser_than_default() {
        let cs = class_of(include_str!("../examples/class.mmd"));
        let o = render::class_opts(&cs, &Opts::default());
        assert!(o.sx <= Opts::default().sx && o.sy <= Opts::default().sy);
        assert!(o.sx > 0.0 && o.sy > 0.0);
    }

    #[test]
    fn class_squeezed_box_keeps_its_name() {
        let cs = class_of(include_str!("../examples/class.mmd"));
        let out = class_preview(&cs, 110, 16);
        for name in ["Customer", "Order", "LineItem", "Payable", "Mailer"] {
            assert!(out.contains(name), "{name}:\n{out}");
        }
    }

    #[test]
    fn class_survives_tiny_areas() {
        let cs = class_of(include_str!("../examples/class.mmd"));
        for (c, r) in [(1, 1), (2, 1), (5, 3), (12, 6), (30, 10)] {
            let out = class_preview(&cs, c, r);
            assert_eq!(out.lines().count(), r as usize);
        }
    }

    #[test]
    fn class_boxes_can_be_focused() {
        let cs = class_of(include_str!("../examples/class.mmd"));
        let opts = Opts {
            highlight: Some(cs.scene.nodes[0].id.clone()),
            ..render::class_opts(&cs, &Opts::default())
        };
        let out = render::preview_class(&cs, 110, 34, &opts);
        assert!(
            out.contains("Customer") && out.contains("+string name"),
            "{out}"
        );
    }

    fn er_of(mmd: &str) -> ErScene {
        match build(mmd, None).unwrap() {
            Built::Er(es) => es,
            _ => panic!("not built as an ER diagram"),
        }
    }

    /// The ER preview at the app's own scale.
    fn er_preview(es: &ErScene, cols: u16, rows: u16) -> String {
        render::preview_er(es, cols, rows, &render::er_opts(es, &Opts::default()))
    }

    #[test]
    fn er_example_draws_columns_keys_cards_and_labels() {
        let es = er_of(include_str!("../examples/er.mmd"));
        let out = er_preview(&es, 110, 34);
        for text in [
            "users",
            "posts",
            "profiles",
            "tags",
            "Category", // the alias, not the id
            "author_id",
            "created_at",
            "PK,FK",
            "login, lower-cased",
            "writes",
            "in",
            "has",
            "tagged",
            "0..*",
            "0..1",
            "1..*",
        ] {
            assert!(out.contains(text), "{text}:\n{out}");
        }
        assert!(!out.contains("categories"), "alias replaces the id:\n{out}");
        // Type, name and key columns line up across a table's rows.
        let col = |needle: &str| {
            let l = out.lines().find(|l| l.contains(needle)).unwrap();
            l.find(needle).unwrap() - l.find("uuid").unwrap()
        };
        assert_eq!(col("author_id"), col("category_id"), "{out}");
    }

    #[test]
    fn er_cards_read_as_text_per_kind() {
        for (rel, card) in [
            ("||--||", "1"),
            ("|o--o|", "0..1"),
            ("}o--o{", "0..*"),
            ("}|--|{", "1..*"),
        ] {
            let es = er_of(&format!("erDiagram\n  a {rel} b : r"));
            let out = er_preview(&es, 60, 30);
            assert_eq!(out.matches(card).count(), 2, "{rel}:\n{out}");
            // Where a line meets a table, the border opens into it.
            assert!(out.contains('┬') && out.contains('┴'), "{rel}:\n{out}");
        }
    }

    #[test]
    fn er_dashes_non_identifying_relations_without_arrowheads() {
        let out = er_preview(&er_of("erDiagram\n  a ||..o{ b : may"), 60, 30);
        assert!(out.contains('╎') || out.contains('╌'), "{out}");
        assert!(!out.contains(['▲', '▼', '▶', '◀']), "{out}");
        let out = er_preview(&er_of("erDiagram\n  a ||--o{ b : must"), 60, 30);
        assert!(!out.contains(['╎', '╌', '▲', '▼', '▶', '◀']), "{out}");
    }

    #[test]
    fn er_scale_is_never_coarser_than_default() {
        let es = er_of(include_str!("../examples/er.mmd"));
        let o = render::er_opts(&es, &Opts::default());
        assert!(o.sx <= Opts::default().sx && o.sy <= Opts::default().sy);
        assert!(o.sx > 0.0 && o.sy > 0.0);
    }

    #[test]
    fn er_squeezed_table_keeps_its_title() {
        let es = er_of(include_str!("../examples/er.mmd"));
        let out = er_preview(&es, 110, 16);
        for name in ["users", "posts", "Category", "profiles", "tags"] {
            assert!(out.contains(name), "{name}:\n{out}");
        }
        assert!(out.contains('…'), "cut rows are marked:\n{out}");
    }

    #[test]
    fn er_survives_tiny_areas() {
        let es = er_of(include_str!("../examples/er.mmd"));
        for (c, r) in [(1, 1), (2, 1), (5, 3), (12, 6), (30, 10)] {
            assert_eq!(er_preview(&es, c, r).lines().count(), r as usize);
        }
        // A table without columns is just its title.
        let out = er_preview(&er_of("erDiagram\n  a ||--o{ b : r"), 60, 30);
        assert!(!out.contains('├'), "{out}");
    }

    #[test]
    fn er_tables_can_be_focused() {
        let es = er_of(include_str!("../examples/er.mmd"));
        let opts = Opts {
            highlight: Some(es.scene.nodes[0].id.clone()),
            ..render::er_opts(&es, &Opts::default())
        };
        let out = render::preview_er(&es, 110, 34, &opts);
        assert!(out.contains("users") && out.contains("created_at"), "{out}");
    }
}
