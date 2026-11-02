//! flowcli — render flowmaid diagrams in the terminal.
//!
//!   flowcli diagram.mmd              # interactive
//!   flowcli diagram.mmd --preview    # print once (headless) and exit
//!   flowcli diagram.mmd --compact 200
//!
//! Interactive keys: q quit · Tab/BackTab focus a node · s save the
//! dragged layout to a `<file>.layout` sidecar (auto-loaded next open)
//! · c toggle compact (serpentine) layout · r reload the file.
//! Mouse: left-click picks whatever is under the cursor — node (focus
//! it), edge, or cluster — via the engine's `Scene::hit_test`; drag a
//! node to move it, edges re-route live via `route_partial` (flowchart
//! and state diagrams).

mod render;

use flowmaid::gitgraph::GitScene;
use flowmaid::model::{Document, Graph};
use flowmaid::scene::Scene;
use render::Opts;
use std::time::Duration;

/// The parsed diagram. `Graph` diagrams (flowchart/state, non-compact)
/// keep their source graph so nodes can be dragged and re-routed live via
/// `route_partial`; everything else is a static `Plain` scene.
enum Built {
    Graph { g: Graph, scene: Scene },
    Plain(Scene),
    GitGraph(GitScene),
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
    /// [`render::git_opts`] for `git_scene`, computed once per rebuild.
    git_opts: Option<Opts>,
    drag: Option<Drag>,
    err: Option<String>,
}

impl App {
    fn rebuild(&mut self) {
        match build(&self.src, self.compact) {
            Ok(built) => {
                let (scene, graph, git_scene) = match built {
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
                            g,
                            base: fresh,
                            base_centers,
                            centers,
                        };
                        (scene, Some(gs), None)
                    }
                    Built::Plain(scene) => (scene, None, None),
                    Built::GitGraph(gs) => (gs.scene.clone(), None, Some(gs)),
                };
                self.ids = scene.nodes.iter().map(|n| n.id.clone()).collect();
                if let Some(f) = self.focus {
                    if f >= self.ids.len() {
                        self.focus = None;
                    }
                }
                self.scene = scene;
                self.graph = graph;
                self.git_opts = git_scene
                    .as_ref()
                    .map(|gs| render::git_opts(gs, &Opts::default()));
                self.git_scene = git_scene;
                self.drag = None;
                self.pick = None; // indices change on rebuild
                self.err = None;
            }
            Err(e) => self.err = Some(e),
        }
    }
    /// Draw/pick options. A gitGraph gets [`render::git_opts`] so draw,
    /// label overlay and click picking all share one finer scale.
    fn opts(&self) -> Opts {
        Opts {
            highlight: self.focus.and_then(|i| self.ids.get(i)).cloned(),
            ..self.git_opts.clone().unwrap_or_default()
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
                Built::Graph { g, scene }
            }
        }),
        Document::Er(d) => Ok(Built::Plain(flowmaid::er::scene(&d).scene)),
        Document::Class(d) => Ok(Built::Plain(flowmaid::class::scene(&d).scene)),
        Document::GitGraph(d) => Ok(Built::GitGraph(flowmaid::gitgraph::scene(&d))),
        Document::Architecture(d) => Ok(Built::Plain(flowmaid::architecture::scene(&d).scene)),
        _ => Err("this diagram type isn't drawable in the terminal yet \
                  (flowchart, state, ER, class, gitGraph, and architecture are)"
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
        pick: None,
        scene: Scene::empty(0.0, 0.0),
        ids: Vec::new(),
        graph: None,
        git_scene: None,
        git_opts: None,
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
        let out = if let Some(gs) = &app.git_scene {
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
            let title = Line::from(vec![
                Span::styled(
                    " flowcli ",
                    Style::default().fg(Color::Black).bg(Color::Cyan),
                ),
                Span::raw(format!(" {} ", app.path)),
                Span::styled(
                    format!(
                        "· {} nodes · {} edges{}{}{} ",
                        app.scene.nodes.len(),
                        app.scene.edges.len(),
                        if app.compact.is_some() {
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
            } else {
                render::draw(f.buffer_mut(), inner, &app.scene, &app.opts());
                if let Some(gs) = &app.git_scene {
                    render::draw_gitgraph_labels(f.buffer_mut(), inner, gs, &app.opts());
                }
            }

            let keycap = Style::default().add_modifier(Modifier::REVERSED);
            let help = Paragraph::new(Line::from(vec![
                Span::styled(" q ", keycap),
                Span::raw(" quit  "),
                Span::styled(" Tab ", keycap),
                Span::raw(" focus node  "),
                Span::styled(" click ", keycap),
                Span::raw(" pick  "),
                Span::styled(" drag ", keycap),
                Span::raw(" move node  "),
                Span::styled(" s ", keycap),
                Span::raw(" save layout  "),
                Span::styled(" c ", keycap),
                Span::raw(" compact  "),
                Span::styled(" r ", keycap),
                Span::raw(" reload"),
            ]))
            .style(Style::default().fg(Color::DarkGray));
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
            Built::Plain(_)
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
}
