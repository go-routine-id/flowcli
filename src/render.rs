//! Draw a flowmaid [`Scene`] into a terminal cell grid.
//!
//! flowmaid hands us plain geometry — node rectangles, edge polylines,
//! cluster boxes, labels, all in f64 pixels — and never assumes a
//! renderer. Here we map those pixels onto character cells and paint
//! them with Unicode box-drawing (`┌─┐│`) for nodes, line-drawing with
//! real junctions (`├┼┴╮`) for edges, and arrowheads (`▶◀▲▼`) at the
//! ends. Custom `style` colours (hex) become true-colour cells.

use flowmaid::class::{ClassBox, ClassRow, ClassScene};
use flowmaid::er::{ErRow, ErScene, ErTable};
use flowmaid::gitgraph::GitScene;
use flowmaid::model::{Card, FrameKind, RelKind, SeqHead, Shape};
use flowmaid::scene::{Scene, SceneNode};
use flowmaid::seq::{PartBox, SeqScene};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

/// Rendering options. `sx`/`sy` are pixels-per-cell on each axis;
/// terminal cells are roughly twice as tall as wide, so `sy ≈ 2·sx`
/// keeps the diagram's proportions. `highlight` dims everything except
/// one node id (drives the interactive "focus a node" mode — this is
/// what `SceneNode.id` from issue #13 is for). `git` draws every node as
/// a one-cell commit glyph on its lane (see [`git_opts`]).
#[derive(Clone)]
pub struct Opts {
    pub sx: f64,
    pub sy: f64,
    pub highlight: Option<String>,
    pub git: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            sx: 8.5,
            sy: 17.0,
            highlight: None,
            git: false,
        }
    }
}

/// Options for drawing a gitGraph. The engine packs commit columns by its
/// own (proportional) measure of the id/tag text (flowmaid ≥ 0.33), which
/// is narrower than one terminal cell per character, so at the default
/// scale neighbouring ids overwrite each other. Picks the coarsest scale
/// (never coarser than `base`) at which, in cells:
/// - neighbouring labels on one row (commit glyphs count as one-character
///   labels) keep a blank cell between them;
/// - no end/middle-anchored label runs off the left edge;
/// - a label never shares a row with another label or a horizontal edge
///   run it overlaps across, when the engine put them at different `y`
///   (it nudges tags off legs by less than a cell's height).
///
/// `sx` follows the first two, `sy` keeps `base`'s `sy/sx` aspect unless
/// the third needs it finer. Compute once per scene (it scans every label
/// pair) and reuse it for draw, overlay and picking.
pub fn git_opts(gs: &GitScene, base: &Opts) -> Opts {
    const GAP_CELLS: f64 = 1.0;
    const SAME_ROW_PX: f64 = 0.5;
    // Cells `round()` can shift a drawn label by, either way.
    const ROUND_SLACK: f64 = 0.5;
    // (x, y, chars, share of the text left of x): start 0, middle ½, end 1.
    // Text labels first, then one entry per commit glyph.
    let items: Vec<(f64, f64, f64, f64)> = gs
        .branch_labels
        .iter()
        .chain(&gs.commit_ids)
        .chain(&gs.tags)
        .map(|l| {
            let lead = match l.anchor {
                "middle" => 0.5,
                "end" => 1.0,
                _ => 0.0,
            };
            (l.x, l.y, l.text.chars().count() as f64, lead)
        })
        .chain(gs.scene.nodes.iter().map(|n| (n.x, n.y, 1.0, 0.5)))
        .collect();
    let mut by_row = items.clone();
    by_row.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.total_cmp(&b.0)));
    // In cells, L spans [xL/sx - leadL·nL, xL/sx + (1-leadL)·nL); R must
    // start a gap cell after L ends: (xR - xL)/sx ≥ (1-leadL)·nL + leadR·nR + 1.
    let between = by_row
        .windows(2)
        .filter(|w| (w[1].1 - w[0].1).abs() < SAME_ROW_PX && w[1].0 > w[0].0)
        .map(|w| {
            let (l, r) = (w[0], w[1]);
            (r.0 - l.0) / ((1.0 - l.3) * l.2 + r.3 * r.2 + GAP_CELLS)
        });
    // Left edge: x/sx - lead·n ≥ ROUND_SLACK.
    let left = items
        .iter()
        .filter(|i| i.3 > 0.0)
        .map(|i| i.0 / (i.3 * i.2 + ROUND_SLACK));
    let sx = between.chain(left).fold(base.sx, f64::min);

    // Horizontal spans in cells at `sx`, with their pixel y. Text labels
    // are checked against every label, commit glyph and horizontal edge
    // run. Glyph-vs-edge and edge-vs-edge are left out: legs leave a
    // commit a few px off its centre, and meet on its row by design.
    let span =
        |&(x, y, n, lead): &(f64, f64, f64, f64)| (x / sx - lead * n, x / sx + (1.0 - lead) * n, y);
    let n_text = items.len() - gs.scene.nodes.len();
    let texts: Vec<_> = items.iter().take(n_text).map(span).collect();
    let others: Vec<_> = items
        .iter()
        .map(span)
        .chain(gs.scene.edges.iter().flat_map(|e| {
            edge_points(e)
                .windows(2)
                .filter(|w| (w[1].1 - w[0].1).abs() < SAME_ROW_PX && w[0].0 != w[1].0)
                .map(|w| (w[0].0.min(w[1].0) / sx, w[0].0.max(w[1].0) / sx, w[0].1))
                .collect::<Vec<_>>()
        }))
        .collect();
    // Overlapping (± rounding) at a different pixel y ⇒ ≥ 1 row apart:
    // |Δy|/sy ≥ 1.
    let sy = texts
        .iter()
        .flat_map(|a| others.iter().map(move |b| (a, b)))
        .filter(|(a, b)| {
            let dy = (a.2 - b.2).abs();
            dy >= SAME_ROW_PX && a.0 < b.1 + ROUND_SLACK && b.0 < a.1 + ROUND_SLACK
        })
        .map(|(a, b)| (a.2 - b.2).abs())
        .fold(sx * base.sy / base.sx, f64::min);
    Opts {
        sx,
        sy,
        git: true,
        ..base.clone()
    }
}

/// The one-cell glyph for a gitGraph commit, by the shape the engine
/// gives it: a merge is a `DoubleCircle`, a HIGHLIGHT a square `Rect`, a
/// REVERSE a `Circle` labelled [`flowmaid::gitgraph::REVERSE_MARK`].
fn commit_glyph(n: &SceneNode) -> char {
    match n.shape {
        Shape::DoubleCircle => '◉',
        Shape::Rect => '■',
        _ if n.label == flowmaid::gitgraph::REVERSE_MARK => '✕',
        _ => '●',
    }
}

// Direction bits for line joining.
const UP: u8 = 1;
const DN: u8 = 2;
const LT: u8 = 4;
const RT: u8 = 8;

/// Box-drawing glyph for a set of connection directions.
fn line_glyph(b: u8) -> char {
    match b {
        x if x == LT | RT => '─',
        x if x == UP | DN => '│',
        x if x == DN | RT => '╭',
        x if x == DN | LT => '╮',
        x if x == UP | RT => '╰',
        x if x == UP | LT => '╯',
        x if x == UP | DN | RT => '├',
        x if x == UP | DN | LT => '┤',
        x if x == LT | RT | DN => '┬',
        x if x == LT | RT | UP => '┴',
        x if x == UP | DN | LT | RT => '┼',
        x if x & (LT | RT) != 0 => '─',
        x if x & (UP | DN) != 0 => '│',
        _ => '·',
    }
}

#[derive(Clone, Copy)]
struct Cell {
    ch: char,
    fg: Color,
    bold: bool,
}

struct Grid {
    cols: usize,
    rows: usize,
    cells: Vec<Option<Cell>>,
    /// Accumulated edge connection bits, before line glyphs are baked.
    bits: Vec<u8>,
}

impl Grid {
    fn new(cols: usize, rows: usize) -> Grid {
        Grid {
            cols,
            rows,
            cells: vec![None; cols * rows],
            bits: vec![0; cols * rows],
        }
    }
    fn idx(&self, c: usize, r: usize) -> Option<usize> {
        (c < self.cols && r < self.rows).then(|| r * self.cols + c)
    }
    fn put(&mut self, c: i64, r: i64, ch: char, fg: Color, bold: bool) {
        if c < 0 || r < 0 {
            return;
        }
        if let Some(i) = self.idx(c as usize, r as usize) {
            self.cells[i] = Some(Cell { ch, fg, bold });
        }
    }
    fn add_bit(&mut self, c: i64, r: i64, bit: u8) {
        if c < 0 || r < 0 {
            return;
        }
        if let Some(i) = self.idx(c as usize, r as usize) {
            self.bits[i] |= bit;
        }
    }
}

/// Draw `scene` into `area` of `buf`. This is the whole adapter — call
/// it from any ratatui frame (or the headless [`preview`] for tests).
pub fn draw(buf: &mut Buffer, area: Rect, scene: &Scene, opts: &Opts) {
    draw_lines(buf, area, scene, opts, true);
}

/// [`draw`], optionally without arrowheads: diagrams whose ends carry
/// their own notation (UML glyphs, cardinalities) draw every edge as a
/// plain line, so a dashed edge's engine kind (`Dotted`, which counts as
/// arrowed) never leaves a head their overlay would have to undo.
fn draw_lines(buf: &mut Buffer, area: Rect, scene: &Scene, opts: &Opts, arrowheads: bool) {
    let cols = area.width as usize;
    let rows = area.height as usize;
    if cols == 0 || rows == 0 {
        return;
    }
    let mut g = Grid::new(cols, rows);

    // Fit the whole diagram into the area: scale so it never overflows,
    // but never magnify past the requested cell size.
    let (sx, sy) = scale(scene, area.width, area.height, opts);
    let cx = |x: f64| (x / sx).round() as i64;
    let cy = |y: f64| (y / sy).round() as i64;
    let dim = opts.highlight.is_some();

    // 1) Cluster boxes (behind everything), rounded + dim.
    for cl in &scene.clusters {
        let (x0, y0) = (cx(cl.x), cy(cl.y));
        let (x1, y1) = (cx(cl.x + cl.w), cy(cl.y + cl.h));
        rect(&mut g, x0, y0, x1, y1, Color::DarkGray, false, false);
        for (k, ch) in cl
            .title
            .chars()
            .take((x1 - x0 - 2).max(0) as usize)
            .enumerate()
        {
            g.put(x0 + 2 + k as i64, y0, ch, Color::DarkGray, false);
        }
    }

    // 1b) Node cell-boxes up front — edges need them for arrowheads,
    // and drawing them last keeps boxes on top of the lines.
    let by_id: std::collections::HashMap<&str, &SceneNode> =
        scene.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut box_of: std::collections::HashMap<&str, (i64, i64, i64, i64)> =
        std::collections::HashMap::new();
    for n in &scene.nodes {
        box_of.insert(n.id.as_str(), cell_box(n, cx, cy));
    }

    // 2) Edges → connection bits (skip invisible ranking links). Collect
    // arrowhead placements for a final pass so node boxes can't clobber
    // them.
    struct Arrow {
        cell: (i64, i64),
        dir: u8,
        col: Color,
        bold: bool,
    }
    let mut arrows: Vec<Arrow> = Vec::new();
    for e in &scene.edges {
        if matches!(e.kind, flowmaid::model::EdgeKind::Invisible) {
            continue;
        }
        let raw = edge_points(e);
        let mut pts: Vec<(i64, i64)> = raw.iter().map(|&(x, y)| (cx(x), cy(y))).collect();
        // Off every box's interior, so an arrowhead never lands on a
        // label (a border cell is fine — heads sit on the frame).
        let clear_of_interiors = |c: (i64, i64)| {
            box_of
                .values()
                .all(|&(x0, y0, x1, y1)| c.0 <= x0 || c.0 >= x1 || c.1 <= y0 || c.1 >= y1)
        };
        // An arrowed edge that ends on a face of its target box gets its
        // head on that face's border cell, pointing square in (issue #6:
        // aiming at the box centre turned heads sideways on wide, short
        // boxes); the line is cut back to meet it.
        let arrowed = arrowheads && e.kind.has_arrow();
        let entry = if arrowed {
            by_id
                .get(e.to.as_str())
                .zip(box_of.get(e.to.as_str()))
                .zip(raw.last())
                .and_then(|((t, &bx), &end)| face_entry(t, bx, end, sx, sy))
                .filter(|&(cell, _)| clear_of_interiors(cell))
        } else {
            None
        };
        if let Some((anchor, _)) = entry {
            let (x0, y0, x1, y1) = box_of[e.to.as_str()];
            while pts.len() > 1
                && pts
                    .last()
                    .is_some_and(|&(c, r)| c >= x0 && c <= x1 && r >= y0 && r <= y1)
            {
                pts.pop();
            }
            pts.push(anchor);
        }
        let faded = dim && !edge_touches(e, opts.highlight.as_deref());
        let col = if faded {
            Color::DarkGray
        } else {
            edge_color(e, &by_id)
        };
        for w in pts.windows(2) {
            walk(&mut g, w[0], w[1]);
        }
        for w in pts.windows(2) {
            colour_run(&mut g, w[0], w[1], col, !faded);
        }
        if let Some((cell, dir)) = entry {
            arrows.push(Arrow {
                cell,
                dir,
                col,
                bold: !faded,
            });
        } else if arrowed {
            // Fallback (no box face, e.g. a cluster target or an end off
            // the frame): the last polyline cell OUTSIDE the target node
            // box, pointing toward it. Placed after nodes are drawn.
            let tbox = box_of.get(e.to.as_str()).copied();
            // Outside the INTERIOR of every node box, so an arrow never
            // lands on another node's label.
            let outside_all = |c: (i64, i64)| {
                box_of
                    .values()
                    .all(|&(x0, y0, x1, y1)| c.0 <= x0 || c.0 >= x1 || c.1 <= y0 || c.1 >= y1)
            };
            if let Some(&anchor) = pts.iter().rev().find(|&&c| outside_all(c)) {
                let target = tbox
                    .map(|(x0, y0, x1, y1)| ((x0 + x1) / 2, (y0 + y1) / 2))
                    .or_else(|| pts.last().copied())
                    .unwrap_or(anchor);
                let d = step_dir(anchor, target);
                if d != 0 {
                    arrows.push(Arrow {
                        cell: anchor,
                        dir: d,
                        col,
                        bold: !faded,
                    });
                }
            }
        }
    }

    // 3) Bake accumulated bits into line glyphs (cells not yet drawn).
    for r in 0..g.rows {
        for c in 0..g.cols {
            let i = r * g.cols + c;
            if g.bits[i] != 0 && g.cells[i].map(|x| x.ch).unwrap_or('·') == '·' {
                let fg = g.cells[i].map(|x| x.fg).unwrap_or(Color::Gray);
                let bold = g.cells[i].map(|x| x.bold).unwrap_or(false);
                g.cells[i] = Some(Cell {
                    ch: line_glyph(g.bits[i]),
                    fg,
                    bold,
                });
            }
        }
    }

    // 4) Node boxes on top (overwrite edges underneath), + labels.
    for n in &scene.nodes {
        let focus = opts.highlight.as_deref() == Some(n.id.as_str());
        let faded = dim && !focus;
        let col = if faded {
            Color::DarkGray
        } else {
            node_color(n)
        };
        let text_col = if faded {
            Color::DarkGray
        } else {
            label_color(n)
        };
        let (x0, y0, x1, y1) = box_of[n.id.as_str()];

        // gitGraph commits: one glyph on the node's own centre cell, so
        // it sits on the lane line (the 2-cell minimum box above would
        // shift a small commit's box centre off the lane). Ids and tags
        // come from the label overlay, not from here.
        if opts.git {
            g.put(cx(n.x), cy(n.y), commit_glyph(n), col, focus);
            continue;
        }

        // Circle nodes (outside gitGraph, which returned above) are drawn
        // as a compact marker with the label underneath.
        if n.shape == Shape::Circle {
            let cx = (x0 + x1) / 2;
            let cy = (y0 + y1) / 2;
            g.put(cx, cy, '●', col, focus);
            let label = flowmaid::layout::spans(n.label.split('\n').next().unwrap_or(""))
                .into_iter()
                .map(|(t, ..)| t)
                .collect::<String>();
            if !label.is_empty() {
                let start = cx - (label.chars().count() as i64) / 2;
                let label_y = y1 + 1;
                for (k, ch) in label.chars().enumerate() {
                    g.put(start + k as i64, label_y, ch, text_col, focus);
                }
            }
            continue;
        }

        let rounded = matches!(
            n.shape,
            Shape::Rounded | Shape::Stadium | Shape::DoubleCircle
        );
        rect(&mut g, x0, y0, x1, y1, col, rounded, focus);
        for r in (y0 + 1)..y1 {
            for c in (x0 + 1)..x1 {
                g.put(c, r, ' ', col, false);
            }
        }
        let inner = (x1 - x0 - 1).max(0) as usize;
        let label = flowmaid::layout::spans(n.label.split('\n').next().unwrap_or(""))
            .into_iter()
            .map(|(t, ..)| t)
            .collect::<String>();
        let label: String = label.chars().take(inner).collect();
        let start = x0 + 1 + ((inner.saturating_sub(label.chars().count())) / 2) as i64;
        let ymid = (y0 + y1) / 2;
        for (k, ch) in label.chars().enumerate() {
            g.put(start + k as i64, ymid, ch, text_col, focus);
        }
    }

    // 5) Arrowheads on top of everything.
    for a in arrows {
        g.put(a.cell.0, a.cell.1, arrow_glyph(a.dir), a.col, a.bold);
    }

    // 6) Blit the grid into the ratatui buffer.
    blit(&g, buf, area);
}

/// A node's cell box `(x0, y0, x1, y1)` (borders included), at least
/// 2×2 so it has an inside. Shared by [`draw`] and the class overlay so
/// both paint the same cells.
fn cell_box(
    n: &SceneNode,
    cx: impl Fn(f64) -> i64,
    cy: impl Fn(f64) -> i64,
) -> (i64, i64, i64, i64) {
    let x0 = cx(n.x - n.w / 2.0);
    let y0 = cy(n.y - n.h / 2.0);
    let x1 = cx(n.x + n.w / 2.0).max(x0 + 2);
    let y1 = cy(n.y + n.h / 2.0).max(y0 + 2);
    (x0, y0, x1, y1)
}

/// Copy the painted cells of `g` into `area` of `buf`.
fn blit(g: &Grid, buf: &mut Buffer, area: Rect) {
    for r in 0..g.rows {
        for c in 0..g.cols {
            if let Some(cell) = g.cells[r * g.cols + c] {
                let x = area.x + c as u16;
                let y = area.y + r as u16;
                if x < area.right() && y < area.bottom() {
                    let mut style = Style::default().fg(cell.fg);
                    if cell.bold {
                        style = style.add_modifier(Modifier::BOLD);
                    }
                    let mut b = [0u8; 4];
                    buf[(x, y)]
                        .set_symbol(cell.ch.encode_utf8(&mut b))
                        .set_style(style);
                }
            }
        }
    }
}

/// Pixels-per-cell used to fit `scene` into an `area` of `cols`×`rows`
/// cells: scale down so it never overflows, but never magnify past the
/// requested cell size. `draw` and [`pick`] MUST share this so a screen
/// click lands on the same scene pixel that was drawn there.
pub fn scale(scene: &Scene, cols: u16, rows: u16, opts: &Opts) -> (f64, f64) {
    let sx = (scene.width / cols.max(1) as f64).max(opts.sx);
    let sy = (scene.height / rows.max(1) as f64).max(opts.sy);
    (sx, sy)
}

/// Map a terminal cell back to a scene pixel. Inverse of `draw`'s
/// `cx`/`cy`: a scene point `x` is drawn at cell `round(x / sx)`, so the
/// scene point at the centre of cell `c` is `c * sx`. `col`/`row` are
/// absolute terminal coordinates and `area` is the inner canvas rect the
/// scene was drawn into. `None` outside the canvas. Zoom/pan never enter
/// the crate — this is the whole screen→scene bridge (drives both picking
/// and drag).
pub fn to_scene(scene: &Scene, area: Rect, col: u16, row: u16, opts: &Opts) -> Option<(f64, f64)> {
    if col < area.x || row < area.y {
        return None;
    }
    let (lc, lr) = (col - area.x, row - area.y);
    if lc >= area.width || lr >= area.height {
        return None;
    }
    let (sx, sy) = scale(scene, area.width, area.height, opts);
    Some((lc as f64 * sx, lr as f64 * sy))
}

/// Pick whatever the terminal cell sits over, via
/// [`flowmaid::scene::Scene::hit_test`]. `None` outside the canvas or on
/// empty space.
pub fn pick(
    scene: &Scene,
    area: Rect,
    col: u16,
    row: u16,
    opts: &Opts,
) -> Option<flowmaid::scene::Hit> {
    let (x, y) = to_scene(scene, area, col, row, opts)?;
    let (sx, sy) = scale(scene, area.width, area.height, opts);
    // One cell of slack so thin edges are still clickable on a coarse grid.
    scene.hit_test(x, y, sx.max(sy))
}

/// The polyline a terminal should trace for one edge: its routed
/// channel waypoints when it has them, else a sampling of its bezier.
fn edge_points(e: &flowmaid::scene::SceneEdge) -> Vec<(f64, f64)> {
    if !e.waypoints.is_empty() {
        return e.waypoints.clone();
    }
    let [p0, p1, p2, p3] = e.bezier;
    (0..=10)
        .map(|i| {
            let t = i as f64 / 10.0;
            let u = 1.0 - t;
            (
                u * u * u * p0.0
                    + 3.0 * u * u * t * p1.0
                    + 3.0 * u * t * t * p2.0
                    + t * t * t * p3.0,
                u * u * u * p0.1
                    + 3.0 * u * u * t * p1.1
                    + 3.0 * u * t * t * p2.1
                    + t * t * t * p3.1,
            )
        })
        .collect()
}

/// 4-connected orthogonal walk between two cells, marking each stepped
/// cell with the connection bits toward its neighbours (so junctions
/// render as `├┼┴` etc. once baked).
fn walk(g: &mut Grid, a: (i64, i64), b: (i64, i64)) {
    let (mut c, mut r) = a;
    while (c, r) != b {
        let (dc, dr) = (b.0 - c, b.1 - r);
        // Step along whichever axis has more distance left (ties → col).
        let (sc, sr) = if dc.abs() >= dr.abs() {
            (dc.signum(), 0)
        } else {
            (0, dr.signum())
        };
        let out = bit(sc, sr);
        g.add_bit(c, r, out);
        c += sc;
        r += sr;
        g.add_bit(c, r, opposite(out));
    }
}

/// Colour the cells of one segment (bits already placed by `walk`),
/// leaving already-solid cells (arrowheads) untouched.
fn colour_run(g: &mut Grid, a: (i64, i64), b: (i64, i64), col: Color, bold: bool) {
    let (mut c, mut r) = a;
    loop {
        if let Some(i) = g.idx(c.max(0) as usize, r.max(0) as usize) {
            if c >= 0 && r >= 0 && g.cells[i].map(|x| x.ch).unwrap_or('·') == '·' {
                let ch = if g.bits[i] != 0 {
                    line_glyph(g.bits[i])
                } else {
                    '·'
                };
                g.cells[i] = Some(Cell { ch, fg: col, bold });
            }
        }
        if (c, r) == b {
            break;
        }
        let (dc, dr) = (b.0 - c, b.1 - r);
        if dc.abs() >= dr.abs() {
            c += dc.signum();
        } else {
            r += dr.signum();
        }
    }
}

fn bit(sc: i64, sr: i64) -> u8 {
    match (sc, sr) {
        (1, 0) => RT,
        (-1, 0) => LT,
        (0, 1) => DN,
        (0, -1) => UP,
        _ => 0,
    }
}
fn opposite(b: u8) -> u8 {
    match b {
        RT => LT,
        LT => RT,
        DN => UP,
        UP => DN,
        _ => 0,
    }
}

/// How far (in cells) an edge's end may sit off a box face and still
/// count as entering through it.
const FACE_SLACK_CELLS: f64 = 0.5;

/// The face of `t` that the edge end `end` (scene px) lies on, as the
/// arrowhead cell on that face's border (in `bx`, the node's cell box)
/// and the direction pointing square into it. `None` when the end
/// is on no face — e.g. a diamond's slanted outline.
fn face_entry(
    t: &SceneNode,
    bx: (i64, i64, i64, i64),
    end: (f64, f64),
    sx: f64,
    sy: f64,
) -> Option<((i64, i64), u8)> {
    let (x0, y0, x1, y1) = bx;
    let (left, right) = (t.x - t.w / 2.0, t.x + t.w / 2.0);
    let (top, bottom) = (t.y - t.h / 2.0, t.y + t.h / 2.0);
    let along_x = end.0 >= left - sx / 2.0 && end.0 <= right + sx / 2.0;
    let along_y = end.1 >= top - sy / 2.0 && end.1 <= bottom + sy / 2.0;
    let faces = [
        ((end.1 - top).abs() / sy, along_x, DN),
        ((end.1 - bottom).abs() / sy, along_x, UP),
        ((end.0 - left).abs() / sx, along_y, RT),
        ((end.0 - right).abs() / sx, along_y, LT),
    ];
    let (_, _, dir) = faces
        .into_iter()
        .filter(|&(off, along, _)| along && off <= FACE_SLACK_CELLS)
        .min_by(|a, b| a.0.total_cmp(&b.0))?;
    // Keep the head off the box's corners (boxes are ≥ 2 cells wide/tall).
    let col = ((end.0 / sx).round() as i64).clamp(x0 + 1, x1 - 1);
    let row = ((end.1 / sy).round() as i64).clamp(y0 + 1, y1 - 1);
    let cell = match dir {
        DN => (col, y0),
        UP => (col, y1),
        RT => (x0, row),
        _ => (x1, row),
    };
    Some((cell, dir))
}

fn step_dir(a: (i64, i64), b: (i64, i64)) -> u8 {
    let (dc, dr) = (b.0 - a.0, b.1 - a.1);
    if dc.abs() >= dr.abs() {
        bit(dc.signum(), 0)
    } else {
        bit(0, dr.signum())
    }
}
fn arrow_glyph(d: u8) -> char {
    match d {
        RT => '▶',
        LT => '◀',
        DN => '▼',
        UP => '▲',
        _ => '•',
    }
}

/// A box outline (rounded or square corners).
#[allow(clippy::too_many_arguments)]
fn rect(g: &mut Grid, x0: i64, y0: i64, x1: i64, y1: i64, col: Color, rounded: bool, bold: bool) {
    let (tl, tr, bl, br) = if rounded {
        ('╭', '╮', '╰', '╯')
    } else {
        ('┌', '┐', '└', '┘')
    };
    for c in (x0 + 1)..x1 {
        g.put(c, y0, '─', col, bold);
        g.put(c, y1, '─', col, bold);
    }
    for r in (y0 + 1)..y1 {
        g.put(x0, r, '│', col, bold);
        g.put(x1, r, '│', col, bold);
    }
    g.put(x0, y0, tl, col, bold);
    g.put(x1, y0, tr, col, bold);
    g.put(x0, y1, bl, col, bold);
    g.put(x1, y1, br, col, bold);
}

fn edge_touches(e: &flowmaid::scene::SceneEdge, id: Option<&str>) -> bool {
    id.is_some_and(|h| e.from == h || e.to == h)
}

fn edge_color(
    e: &flowmaid::scene::SceneEdge,
    by_id: &std::collections::HashMap<&str, &SceneNode>,
) -> Color {
    // Tint an edge by its source node's colour — reads as flow.
    by_id
        .get(e.from.as_str())
        .map(|n| node_color(n))
        .unwrap_or(Color::Gray)
}

fn node_color(n: &SceneNode) -> Color {
    n.style
        .stroke
        .as_deref()
        .and_then(hex)
        .unwrap_or_else(|| shape_color(n.shape))
}
fn label_color(n: &SceneNode) -> Color {
    n.style
        .color
        .as_deref()
        .and_then(hex)
        .unwrap_or(Color::White)
}

fn shape_color(s: Shape) -> Color {
    match s {
        Shape::Diamond => Color::Yellow,
        Shape::Circle | Shape::DoubleCircle => Color::Magenta,
        Shape::Hexagon | Shape::Cylinder => Color::Blue,
        Shape::Stadium | Shape::StateStart | Shape::StateEnd => Color::Green,
        Shape::ForkBar => Color::Gray,
        _ => Color::Cyan,
    }
}

/// Parse `#rrggbb` (or `#rgb`) into a true-colour value.
fn hex(s: &str) -> Option<Color> {
    let h = s.strip_prefix('#')?;
    let (r, g, b) = match h.len() {
        6 => (
            u8::from_str_radix(&h[0..2], 16).ok()?,
            u8::from_str_radix(&h[2..4], 16).ok()?,
            u8::from_str_radix(&h[4..6], 16).ok()?,
        ),
        3 => {
            let d = |c: &str| u8::from_str_radix(c, 16).map(|v| v * 17).ok();
            (d(&h[0..1])?, d(&h[1..2])?, d(&h[2..3])?)
        }
        _ => return None,
    };
    Some(Color::Rgb(r, g, b))
}

/// Overlay git-specific labels (`branch_labels`, `commit_ids`, `tags`) on
/// top of the generic scene drawing. Uses the same pixel→cell scale as
/// [`draw`] so the text lines up with the glyphs and lanes from `gs.scene`.
///
/// When the graph is wider than the canvas, [`scale`] squeezes it to fit
/// and ids no longer have room. Labels then claim cells in priority order
/// — branch names, tags, the focused commit's id, then the other ids left
/// to right — and an id that runs into a claimed cell (or the one-cell gap
/// after it) is cut to `…` if at least [`MIN_ELIDED_CELLS`] cells are
/// free, else left out. Focus a commit to read its full id.
pub fn draw_gitgraph_labels(buf: &mut Buffer, area: Rect, gs: &GitScene, opts: &Opts) {
    let cols = area.width as usize;
    let rows = area.height as usize;
    if cols == 0 || rows == 0 {
        return;
    }
    let (sx, sy) = scale(&gs.scene, area.width, area.height, opts);
    let cx = |x: f64| (x / sx).round() as i64;
    let cy = |y: f64| (y / sy).round() as i64;

    let focused = |l: &&flowmaid::gitgraph::GitLabel| opts.highlight.as_deref() == Some(&l.text);
    let mut ids: Vec<_> = gs.commit_ids.iter().collect();
    ids.sort_by(|a, b| a.x.total_cmp(&b.x).then(a.y.total_cmp(&b.y)));
    let (focus, rest): (Vec<_>, Vec<_>) = ids.into_iter().partition(focused);
    let must = gs.branch_labels.iter().chain(&gs.tags).chain(focus);

    // Cells already taken by a drawn label, area-relative.
    let mut taken = vec![false; cols * rows];
    // Left of the canvas counts as free (nothing is drawn there); past its
    // right or bottom edge does not, so an id running off it is cut to `…`.
    let free = |taken: &[bool], c: i64, r: i64| {
        c < 0
            || (r >= 0
                && c < cols as i64
                && r < rows as i64
                && !taken[r as usize * cols + c as usize])
    };
    let mut place = |l: &flowmaid::gitgraph::GitLabel, may_elide: bool| {
        let n = l.text.chars().count() as i64;
        let x = match l.anchor {
            "middle" => cx(l.x) - n / 2,
            "end" => cx(l.x) - n,
            _ => cx(l.x), // start
        };
        let y = cy(l.y);
        let text: String = if may_elide {
            // Free run from `x`, and the cells it may use while keeping a
            // gap cell before whatever is claimed next.
            let run = (0..n + ID_GAP_CELLS)
                .take_while(|&k| free(&taken, x + k, y))
                .count() as i64;
            let room = (run - ID_GAP_CELLS).min(n);
            if !free(&taken, x - ID_GAP_CELLS, y) || room < MIN_ELIDED_CELLS.min(n) {
                return;
            }
            if room == n {
                l.text.clone()
            } else {
                l.text
                    .chars()
                    .take(room as usize - 1)
                    .chain(['…'])
                    .collect()
            }
        } else {
            l.text.clone()
        };
        let col = hex(l.color).unwrap_or(Color::White);
        let style = Style::default()
            .fg(col)
            .add_modifier(if l.font_weight.is_some() {
                Modifier::BOLD
            } else {
                Modifier::empty()
            });
        for (k, ch) in text.chars().enumerate() {
            let (c, r) = (x + k as i64, y);
            if c < 0 || r < 0 || c >= cols as i64 || r >= rows as i64 {
                continue;
            }
            taken[r as usize * cols + c as usize] = true;
            let mut b = [0u8; 4];
            buf[(area.x + c as u16, area.y + r as u16)]
                .set_symbol(ch.encode_utf8(&mut b))
                .set_style(style);
        }
    };
    for l in must {
        place(l, false);
    }
    for l in rest {
        place(l, true);
    }
}

/// Blank cells kept between an id and the label before or after it.
const ID_GAP_CELLS: i64 = 1;
/// Narrowest an id may be cut to: one character plus `…`.
const MIN_ELIDED_CELLS: i64 = 2;

/// Headless render to a plain-text block — for tests and `--preview`,
/// so the terminal output is verifiable without an interactive TTY.
pub fn preview(scene: &Scene, cols: u16, rows: u16, opts: &Opts) -> String {
    render_text(cols, rows, |buf, area| draw(buf, area, scene, opts))
}

/// Run `paint` on a headless `cols`×`rows` terminal and return its
/// cells as plain text, one line per row.
fn render_text(cols: u16, rows: u16, paint: impl FnOnce(&mut Buffer, Rect)) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    term.draw(|f| {
        let area = f.area();
        paint(f.buffer_mut(), area);
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..rows {
        let mut x = 0;
        while x < cols {
            let sym = buf[(x, y)].symbol();
            out.push_str(sym);
            // A wide glyph covers the next cell too; skip that cell so the
            // text line keeps the grid's width.
            x += (cells(sym) as u16).max(1);
        }
        out.push('\n');
    }
    out
}

/// Headless render of a git graph, including branch labels and tags.
/// `opts` should come from [`git_opts`] (as the app's do).
pub fn preview_with_git(gs: &GitScene, cols: u16, rows: u16, opts: &Opts) -> String {
    render_text(cols, rows, |buf, area| {
        draw(buf, area, &gs.scene, opts);
        draw_gitgraph_labels(buf, area, gs, opts);
    })
}

// ---------------------------------------------------------------------
// Sequence diagrams
// ---------------------------------------------------------------------

/// Colours of the sequence painter, kept together so the palette reads
/// as one theme: guides (lifelines, frames) recede, messages and labels
/// stand out, actors are told apart from plain participants.
const SEQ_GUIDE: Color = Color::DarkGray;
const SEQ_PART: Color = Color::Cyan;
const SEQ_ACTOR: Color = Color::Magenta;
const SEQ_LINE: Color = Color::Gray;
const SEQ_TEXT: Color = Color::White;
const SEQ_NUMBER: Color = Color::Yellow;
const SEQ_NOTE: Color = Color::Yellow;
const SEQ_ACTIVATION: Color = Color::Cyan;
const SEQ_FRAME_LABEL: Color = Color::Gray;

/// Pixels-per-cell used to fit a sequence diagram into `cols`×`rows`:
/// the same rule as [`scale`], over [`SeqScene`]'s own extent.
pub fn seq_scale(sc: &SeqScene, cols: u16, rows: u16, opts: &Opts) -> (f64, f64) {
    let sx = (sc.width / cols.max(1) as f64).max(opts.sx);
    let sy = (sc.height / rows.max(1) as f64).max(opts.sy);
    (sx, sy)
}

/// Plain text of a (possibly multi-line, possibly `<b>`-tagged) label,
/// on one line — terminal cells hold one row of text per element.
fn flat_label(s: &str) -> String {
    s.split('\n')
        .map(|l| {
            flowmaid::layout::spans(l)
                .into_iter()
                .map(|(t, ..)| t)
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Write `text` starting at cell `(c, r)`, clipped to the grid.
fn put_text(g: &mut Grid, c: i64, r: i64, text: &str, fg: Color, bold: bool) {
    for (k, ch) in text.chars().enumerate() {
        g.put(c + k as i64, r, ch, fg, bold);
    }
}

/// Cell box `(x0, y0, x1, y1)` for a scene rectangle, grown (about its
/// centre) to hold `text_cells` of text on its middle row and at least
/// one inner row — the engine sizes boxes by proportional text width,
/// which is narrower than one cell per character.
fn text_box(
    (x, y, w, h): (f64, f64, f64, f64),
    text_cells: i64,
    cx: impl Fn(f64) -> i64,
    cy: impl Fn(f64) -> i64,
) -> (i64, i64, i64, i64) {
    let (mut x0, mut x1) = (cx(x), cx(x + w));
    let need = text_cells + 3; // two borders + a blank cell each side, minus one
    if x1 - x0 < need {
        let grow = need - (x1 - x0);
        x0 -= grow / 2;
        x1 += grow - grow / 2;
    }
    let y0 = cy(y);
    let y1 = cy(y + h).max(y0 + 2);
    (x0, y0, x1, y1)
}

/// Head glyph for a message pointing right (`right`) or left.
fn seq_head_glyph(head: SeqHead, right: bool) -> Option<char> {
    match head {
        SeqHead::Filled => Some(if right { '▶' } else { '◀' }),
        SeqHead::Open | SeqHead::Async => Some(if right { '▷' } else { '◁' }),
        SeqHead::Cross => Some('✕'),
        SeqHead::None => None,
    }
}

/// Draw a sequence diagram into `area` of `buf`. Static: there is
/// nothing to pick or drag, so this is the whole adapter.
pub fn draw_seq(buf: &mut Buffer, area: Rect, sc: &SeqScene, opts: &Opts) {
    let cols = area.width as usize;
    let rows = area.height as usize;
    if cols == 0 || rows == 0 {
        return;
    }
    let mut g = Grid::new(cols, rows);
    let (sx, sy) = seq_scale(sc, area.width, area.height, opts);
    let cx = |x: f64| (x / sx).round() as i64;
    let cy = |y: f64| (y / sy).round() as i64;

    // Participant cell boxes, header and footer, computed up front:
    // lifelines run between them.
    // An actor's slot is taller to fit its stick figure, which a cell
    // grid can't draw: give it a plain box's height (same top edge) so
    // it doesn't swallow the first message row.
    let part_box = |b: &PartBox| {
        let label = flat_label(&b.label);
        let h = b.h.min(flowmaid::seq::BOX_H);
        let bx = text_box((b.x, b.y, b.w, h), label.chars().count() as i64, cx, cy);
        (bx, label)
    };
    let heads: Vec<_> = sc.boxes.iter().map(part_box).collect();
    let feet: Vec<_> = sc.footer.iter().map(part_box).collect();
    let life_col = |i: usize| sc.lifelines.get(i).map(|l| cx(l.x));

    // 1) Lifelines (dashed, behind everything).
    for (i, l) in sc.lifelines.iter().enumerate() {
        let c = cx(l.x);
        let top = heads.get(i).map(|(b, _)| b.3 + 1).unwrap_or(cy(l.y0));
        let bottom = feet.get(i).map(|(b, _)| b.1 - 1).unwrap_or(cy(l.y1));
        for r in top..=bottom {
            g.put(c, r, '┆', SEQ_GUIDE, false);
        }
    }

    // 2) Frames: dim border, `[kind] label` chip on the top edge,
    // `┄` dividers with their own `[else]` / `[and]` chip.
    for f in &sc.frames {
        let (x0, y0) = (cx(f.x), cy(f.y));
        let (x1, y1) = (cx(f.x + f.w), cy(f.y + f.h).max(cy(f.y) + 1));
        rect(&mut g, x0, y0, x1, y1, SEQ_GUIDE, false, false);
        let inner = (x1 - x0 - 1).max(0) as usize;
        let chip = |kw: &str, label: &str| {
            let label = flat_label(label);
            let s = if label.is_empty() {
                format!("[{kw}]")
            } else {
                format!("[{kw}] {label}")
            };
            s.chars().take(inner).collect::<String>()
        };
        put_text(
            &mut g,
            x0 + 1,
            y0,
            &chip(f.kind.keyword(), &f.label),
            SEQ_FRAME_LABEL,
            true,
        );
        let divider_kw = match f.kind {
            FrameKind::Par => "and",
            _ => "else",
        };
        for (dy, label) in &f.dividers {
            let r = cy(*dy);
            g.put(x0, r, '├', SEQ_GUIDE, false);
            for c in (x0 + 1)..x1 {
                g.put(c, r, '┄', SEQ_GUIDE, false);
            }
            g.put(x1, r, '┤', SEQ_GUIDE, false);
            put_text(
                &mut g,
                x0 + 1,
                r,
                &chip(divider_kw, label),
                SEQ_FRAME_LABEL,
                true,
            );
        }
    }

    // 3) Activation bars over the lifelines.
    for a in &sc.activations {
        let c = cx(a.x);
        for r in cy(a.y0)..=cy(a.y1) {
            g.put(c, r, '┃', SEQ_ACTIVATION, false);
        }
    }

    // 4) Messages: line, head, label (with the autonumber prefix).
    for m in &sc.messages {
        let (Some(&first), Some(&last)) = (m.points.first(), m.points.last()) else {
            continue;
        };
        let (h, v) = if m.dashed {
            ('╌', '╎')
        } else {
            ('─', '│')
        };
        let label = {
            let text = flat_label(&m.text);
            match m.number {
                Some(n) if text.is_empty() => format!("{n}."),
                Some(n) => format!("{n}. {text}"),
                None => text,
            }
        };
        let numbered = m
            .number
            .map(|n| format!("{n}.").chars().count())
            .unwrap_or(0);
        let put_label = |g: &mut Grid, start: i64, r: i64| {
            for (k, ch) in label.chars().enumerate() {
                let (fg, bold) = if k < numbered {
                    (SEQ_NUMBER, true)
                } else {
                    (SEQ_TEXT, false)
                };
                g.put(start + k as i64, r, ch, fg, bold);
            }
        };
        if m.points.len() >= 4 {
            // Self-message: out right, down, back into the lifeline.
            let s = life_col(m.from).unwrap_or(cx(first.0));
            let c1 = cx(m.points[1].0).max(s + 2);
            let r0 = cy(first.1);
            let r1 = cy(last.1).max(r0 + 1);
            for c in (s + 1)..c1 {
                g.put(c, r0, h, SEQ_LINE, false);
                g.put(c, r1, h, SEQ_LINE, false);
            }
            g.put(c1, r0, '╮', SEQ_LINE, false);
            for r in (r0 + 1)..r1 {
                g.put(c1, r, v, SEQ_LINE, false);
            }
            g.put(c1, r1, '╯', SEQ_LINE, false);
            if let Some(hc) = seq_head_glyph(m.head, false) {
                g.put(s + 1, r1, hc, SEQ_TEXT, true);
            }
            put_label(&mut g, c1 + 2, r0);
            continue;
        }
        let s = life_col(m.from).unwrap_or(cx(first.0));
        let t = life_col(m.to).unwrap_or(cx(last.0));
        let right = t >= s;
        let dir = if right { 1 } else { -1 };
        let r = cy(first.1);
        // The line runs between the two lifelines, which stay visible.
        let (a, b) = (s + dir, t - dir);
        let (lo, hi) = (a.min(b), a.max(b));
        for c in lo..=hi {
            g.put(c, r, h, SEQ_LINE, false);
        }
        if let Some(hc) = seq_head_glyph(m.head, right) {
            g.put(b, r, hc, SEQ_TEXT, true);
        }
        // Label on the row above the line, centred on the engine's anchor.
        let lr = cy(m.label_pos.1).min(r - 1);
        let w = label.chars().count() as i64;
        let start = if m.label_centered {
            cx(m.label_pos.0) - w / 2
        } else {
            cx(m.label_pos.0)
        };
        put_label(&mut g, start, lr);
    }

    // 5) Notes: yellow cards — or, when the note is under two rows
    // tall at this scale, a one-row `▌ text ▐` band so the card's
    // borders don't spill onto the rows around it.
    for n in &sc.notes {
        let text = flat_label(&n.text);
        let w = text.chars().count() as i64;
        if cy(n.y + n.h) - cy(n.y) < 2 {
            let start = cx(n.x + n.w / 2.0) - (w + 4) / 2;
            let r = cy(n.y + n.h / 2.0);
            put_text(&mut g, start, r, &format!("▌ {text} ▐"), SEQ_NOTE, false);
            continue;
        }
        let (x0, y0, x1, y1) = text_box((n.x, n.y, n.w, n.h), w, cx, cy);
        rect(&mut g, x0, y0, x1, y1, SEQ_NOTE, false, false);
        for r in (y0 + 1)..y1 {
            for c in (x0 + 1)..x1 {
                g.put(c, r, ' ', SEQ_NOTE, false);
            }
        }
        let inner = (x1 - x0 - 1).max(0);
        let start = x0 + 1 + (inner - text.chars().count() as i64).max(0) / 2;
        put_text(&mut g, start, (y0 + y1) / 2, &text, SEQ_NOTE, false);
    }

    // 6) Participant boxes on top, header and footer; the lifeline
    // joins each one with a `┬` / `┴` on its border.
    let parts = sc.boxes.iter().zip(&heads).map(|(p, b)| (p, b, true));
    let footers = sc.footer.iter().zip(&feet).map(|(p, b)| (p, b, false));
    for (i, (p, ((x0, y0, x1, y1), label), header)) in parts.enumerate().chain(footers.enumerate())
    {
        let (x0, y0, x1, y1) = (*x0, *y0, *x1, *y1);
        let col = if p.actor { SEQ_ACTOR } else { SEQ_PART };
        rect(&mut g, x0, y0, x1, y1, col, p.actor, true);
        for r in (y0 + 1)..y1 {
            for c in (x0 + 1)..x1 {
                g.put(c, r, ' ', col, false);
            }
        }
        let inner = (x1 - x0 - 1).max(0);
        let start = x0 + 1 + (inner - label.chars().count() as i64).max(0) / 2;
        put_text(&mut g, start, (y0 + y1) / 2, label, SEQ_TEXT, true);
        if let Some(c) = life_col(i) {
            if c > x0 && c < x1 {
                if header {
                    g.put(c, y1, '┬', col, true);
                } else {
                    g.put(c, y0, '┴', col, true);
                }
            }
        }
    }

    blit(&g, buf, area);
}

/// Headless render of a sequence diagram — `--preview` and tests.
pub fn preview_seq(sc: &SeqScene, cols: u16, rows: u16, opts: &Opts) -> String {
    render_text(cols, rows, |buf, area| draw_seq(buf, area, sc, opts))
}

// ---------------------------------------------------------------------
// Class diagrams
// ---------------------------------------------------------------------

/// Colours of the class overlay. Box borders and lines keep the generic
/// painter's colours (the node's `style`, else its shape's); these are
/// for the text it adds.
const CLASS_ANNOTATION: Color = Color::Magenta;
const CLASS_MEMBER: Color = Color::Gray;
const CLASS_REL_LABEL: Color = Color::White;
const CLASS_CARD: Color = Color::LightBlue;
const CLASS_NOTE: Color = Color::Yellow;

/// Cells a box's outline can lose to rounding its two faces separately,
/// on top of the two border cells (both axes).
const BOX_FRAME_CELLS: f64 = 2.0;
/// Blank cells between a member row and the box's side borders (one
/// each side).
const BOX_TEXT_PAD_CELLS: f64 = 2.0;

/// One text row of a class box, top to bottom.
enum ClassLine<'a> {
    Annotation(&'a str),
    Name,
    Divider,
    Member(&'a ClassRow),
    /// Stands in for the rows cut when the box is squeezed.
    More,
}

/// The rows a class box is drawn with: stereotypes, the name, then the
/// attribute and operation compartments, each opened by a divider. The
/// attribute compartment is kept (empty) when only operations exist,
/// so operations always sit in the second one, as in UML.
fn class_lines(b: &ClassBox) -> Vec<ClassLine<'_>> {
    let mut v: Vec<ClassLine> = b
        .annotations
        .iter()
        .map(|a| ClassLine::Annotation(a))
        .collect();
    v.push(ClassLine::Name);
    if !b.fields.is_empty() || !b.methods.is_empty() {
        v.push(ClassLine::Divider);
        v.extend(b.fields.iter().map(ClassLine::Member));
    }
    if !b.methods.is_empty() {
        v.push(ClassLine::Divider);
        v.extend(b.methods.iter().map(ClassLine::Member));
    }
    v
}

/// [`class_lines`] cut to `room` rows for a squeezed box, least
/// telling first: the empty attribute compartment, then stereotypes,
/// then trailing members (an `…` row marks the cut). The name stays.
fn class_lines_within(b: &ClassBox, room: usize) -> Vec<ClassLine<'_>> {
    let mut v = class_lines(b);
    if room <= 1 {
        return vec![ClassLine::Name];
    }
    if v.len() > room && b.fields.is_empty() && !b.methods.is_empty() {
        if let Some(k) = v.iter().position(|l| matches!(l, ClassLine::Divider)) {
            v.remove(k);
        }
    }
    if v.len() > room {
        v.retain(|l| !matches!(l, ClassLine::Annotation(_)));
    }
    if v.len() > room {
        v.truncate(room - 1);
        while matches!(v.last(), Some(ClassLine::Divider)) {
            v.pop();
        }
        v.push(ClassLine::More);
    }
    v
}

/// A class's name as drawn: verbatim on one line. Not [`flat_label`] —
/// a generic `Box<I>` or `Repo<B>` would lose its parameter as an
/// `<i>`/`<b>` tag.
fn class_name(b: &ClassBox) -> String {
    b.name.replace('\n', " ")
}

fn class_line_text<'a>(b: &'a ClassBox, l: &ClassLine<'a>) -> &'a str {
    match l {
        ClassLine::Annotation(a) => a,
        ClassLine::Name => &b.name,
        ClassLine::Divider => "",
        ClassLine::Member(r) => &r.text,
        ClassLine::More => "…",
    }
}

/// Options for drawing a class diagram. The engine sizes a box by its
/// proportional text width and 20 px per member row, both finer than a
/// terminal cell at the default scale, so members would be cut or not
/// drawn at all. Picks the coarsest scale (never coarser than `base`)
/// at which every class box gives each of its rows one cell row and its
/// widest row a padded fit, and every note fits its lines the same way.
/// The two axes are fitted separately. Compute once per scene and reuse
/// it for draw and picking.
pub fn class_opts(cs: &ClassScene, base: &Opts) -> Opts {
    // (node, widest row in cells, rows, frame + padding cells).
    let classes = cs.boxes.iter().zip(&cs.scene.nodes).map(|(b, n)| {
        if b.lollipop {
            // Frameless: just the name on one row.
            return (n, cells(&class_name(b)), 1, 0.0);
        }
        let lines = class_lines(b);
        let widest = lines
            .iter()
            .map(|l| cells(class_line_text(b, l)))
            .max()
            .unwrap_or(0);
        (n, widest, lines.len(), BOX_FRAME_CELLS)
    });
    let notes = cs
        .notes
        .iter()
        .zip(cs.scene.nodes.iter().skip(cs.boxes.len()))
        .map(|(nb, n)| {
            let widest = nb.text.split('\n').map(cells).max();
            let rows = nb.text.split('\n').count();
            (n, widest.unwrap_or(0), rows, BOX_FRAME_CELLS)
        });
    let (mut sx, mut sy) = (base.sx, base.sy);
    for (n, widest, rows, frame) in classes.chain(notes) {
        let pad = if frame > 0.0 { BOX_TEXT_PAD_CELLS } else { 0.0 };
        if n.w > 0.0 && widest > 0 {
            sx = sx.min(n.w / (widest as f64 + pad + frame));
        }
        if n.h > 0.0 {
            sy = sy.min(n.h / (rows as f64 + frame));
        }
    }
    Opts {
        sx,
        sy,
        ..base.clone()
    }
}

/// The end glyph of a relationship entering a box in direction `dir`
/// (pointing into it): hollow triangles for inheritance/realization,
/// diamonds for composition/aggregation, filled arrows for
/// association/dependency, a circle for a lollipop; a plain link has none.
fn rel_glyph(kind: RelKind, dir: u8) -> Option<char> {
    use RelKind::*;
    match kind {
        Inheritance | Realization => Some(match dir {
            DN => '▽',
            UP => '△',
            RT => '▷',
            _ => '◁',
        }),
        Composition => Some('◆'),
        Aggregation => Some('◇'),
        Association | Dependency => Some(arrow_glyph(dir)),
        Lollipop => Some('○'),
        Link => None,
    }
}

/// The border glyph where a line meets a box face, entering in `dir`.
fn face_junction(dir: u8) -> char {
    match dir {
        DN => '┴',
        UP => '┬',
        RT => '┤',
        _ => '├',
    }
}

/// Cell-addressed writes into `area` of a ratatui buffer, clipped to it.
struct Canvas<'a> {
    buf: &'a mut Buffer,
    area: Rect,
}

impl Canvas<'_> {
    fn at(&self, c: i64, r: i64) -> Option<(u16, u16)> {
        let (x, y) = (self.area.x as i64 + c, self.area.y as i64 + r);
        (c >= 0 && r >= 0 && x < self.area.right() as i64 && y < self.area.bottom() as i64)
            .then_some((x as u16, y as u16))
    }
    fn symbol(&self, c: i64, r: i64) -> Option<&str> {
        self.at(c, r).map(|p| self.buf[p].symbol())
    }
    fn blank(&self, c: i64, r: i64) -> bool {
        self.symbol(c, r) == Some(" ")
    }
    fn put(&mut self, c: i64, r: i64, ch: char, style: Style) {
        if let Some(p) = self.at(c, r) {
            let mut b = [0u8; 4];
            self.buf[p]
                .set_symbol(ch.encode_utf8(&mut b))
                .set_style(style);
        }
    }
    /// Swap the symbol only, keeping the cell's colours.
    fn swap(&mut self, c: i64, r: i64, ch: char) {
        if let Some(p) = self.at(c, r) {
            let mut b = [0u8; 4];
            self.buf[p].set_symbol(ch.encode_utf8(&mut b));
        }
    }
    /// Write `s` from `(c, r)`: a wide character takes its cell and
    /// blanks the next, as ratatui lays it; zero-width ones are dropped.
    fn text(&mut self, c: i64, r: i64, s: &str, style: Style) {
        let mut at = c;
        for ch in s.chars() {
            let w = char_cells(ch);
            if w == 0 {
                continue;
            }
            // A wide glyph clipped by the right edge would spill past it.
            if w == 2 && self.at(at + 1, r).is_none() {
                break;
            }
            self.put(at, r, ch, style);
            if w == 2 {
                // Its second cell is hidden behind it — reset, as ratatui does.
                if let Some(p) = self.at(at + 1, r) {
                    self.buf[p].reset();
                }
            }
            at += w;
        }
    }
}

fn styled(fg: Color, bold: bool) -> Style {
    let s = Style::default().fg(fg);
    if bold {
        s.add_modifier(Modifier::BOLD)
    } else {
        s
    }
}

/// Terminal cells `ch` takes: two for wide (CJK, emoji) characters,
/// none for zero-width ones.
fn char_cells(ch: char) -> i64 {
    cells(ch.encode_utf8(&mut [0u8; 4]))
}

/// Terminal cells `s` takes.
fn cells(s: &str) -> i64 {
    Span::raw(s).width() as i64
}

/// `s` cut to `cells` terminal cells, the last one `…` when cut.
fn fit(s: &str, room: i64) -> String {
    if cells(s) <= room {
        return s.to_string();
    }
    if room <= 0 {
        return String::new();
    }
    let mut out = String::new();
    let mut used = 0;
    for ch in s.chars() {
        let w = char_cells(ch);
        if used + w > room - 1 {
            break;
        }
        out.push(ch);
        used += w;
    }
    out.push('…');
    out
}

/// The cells an edge's polyline steps through, in order — the same
/// walk [`draw`] lays its line along.
fn path_cells(pts: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out: Vec<(i64, i64)> = pts.first().copied().into_iter().collect();
    for w in pts.windows(2) {
        let (mut c, mut r) = w[0];
        while (c, r) != w[1] {
            let (dc, dr) = (w[1].0 - c, w[1].1 - r);
            if dc.abs() >= dr.abs() {
                c += dc.signum();
            } else {
                r += dr.signum();
            }
            out.push((c, r));
        }
    }
    out
}

/// Where an edge meets a box: the cell and the direction pointing in.
type FaceEnd = ((i64, i64), u8);

/// Cell geometry the class and ER overlays share: the scale, every
/// box's cells, and every edge's cell polyline and the cells it steps
/// through — the same walk [`draw`] lays its lines along.
struct Geo<'s> {
    sc: &'s Scene,
    sx: f64,
    sy: f64,
    boxes: Vec<(i64, i64, i64, i64)>,
    index: std::collections::HashMap<&'s str, usize>,
    /// Per edge: its polyline in cells, and its first/last scene point.
    pts: Vec<Vec<(i64, i64)>>,
    raw_ends: Vec<Option<[(f64, f64); 2]>>,
    paths: Vec<Vec<(i64, i64)>>,
}

impl<'s> Geo<'s> {
    fn new(sc: &'s Scene, area: Rect, opts: &Opts) -> Self {
        let (sx, sy) = scale(sc, area.width, area.height, opts);
        let cx = |x: f64| (x / sx).round() as i64;
        let cy = |y: f64| (y / sy).round() as i64;
        let mut pts = Vec::with_capacity(sc.edges.len());
        let mut raw_ends = Vec::with_capacity(sc.edges.len());
        for e in &sc.edges {
            let raw = edge_points(e);
            raw_ends.push(raw.first().zip(raw.last()).map(|(&a, &b)| [a, b]));
            pts.push(raw.iter().map(|&(x, y)| (cx(x), cy(y))).collect::<Vec<_>>());
        }
        Geo {
            sc,
            sx,
            sy,
            boxes: sc.nodes.iter().map(|n| cell_box(n, cx, cy)).collect(),
            index: sc
                .nodes
                .iter()
                .enumerate()
                .map(|(i, n)| (n.id.as_str(), i))
                .collect(),
            paths: pts.iter().map(|p| path_cells(p)).collect(),
            pts,
            raw_ends,
        }
    }
    fn cx(&self, x: f64) -> i64 {
        (x / self.sx).round() as i64
    }
    fn cy(&self, y: f64) -> i64 {
        (y / self.sy).round() as i64
    }
    /// On a box, border included.
    fn in_box(&self, c: (i64, i64)) -> bool {
        self.boxes
            .iter()
            .any(|&(x0, y0, x1, y1)| c.0 >= x0 && c.0 <= x1 && c.1 >= y0 && c.1 <= y1)
    }
    /// Off every box's interior (a border cell is fine).
    fn outside_interiors(&self, c: (i64, i64)) -> bool {
        self.boxes
            .iter()
            .all(|&(x0, y0, x1, y1)| c.0 <= x0 || c.0 >= x1 || c.1 <= y0 || c.1 >= y1)
    }
    /// The colour [`draw`] gave edge `e`'s line: its source node's.
    fn line_color(&self, e: &flowmaid::scene::SceneEdge) -> Color {
        self.index
            .get(e.from.as_str())
            .map(|&i| node_color(&self.sc.nodes[i]))
            .unwrap_or(Color::Gray)
    }
    /// Dash the lines of the edges `dashed` picks, outside the boxes.
    fn dash(&self, cv: &mut Canvas, dashed: impl Fn(&flowmaid::scene::SceneEdge) -> bool) {
        for (e, path) in self.sc.edges.iter().zip(&self.paths) {
            if !dashed(e) {
                continue;
            }
            for &(c, r) in path.iter().filter(|&&c| !self.in_box(c)) {
                match cv.symbol(c, r) {
                    Some("─") => cv.swap(c, r, '╌'),
                    Some("│") => cv.swap(c, r, '╎'),
                    _ => {}
                }
            }
        }
    }
    /// Where edge `j` meets its two boxes: the cell, the direction
    /// pointing in, and whether that cell is on the box's face (where
    /// [`draw`] would put an arrowhead) rather than the polyline's last
    /// cell off every box interior.
    fn ends(&self, j: usize) -> [Option<(FaceEnd, bool)>; 2] {
        let (Some(e), Some(Some(raw)), Some(pts)) =
            (self.sc.edges.get(j), self.raw_ends.get(j), self.pts.get(j))
        else {
            return [None, None];
        };
        let mut pair = [None, None];
        for (side, id) in [&e.from, &e.to].into_iter().enumerate() {
            let Some(&i) = self.index.get(id.as_str()) else {
                continue;
            };
            let face = face_entry(
                &self.sc.nodes[i],
                self.boxes[i],
                raw[side],
                self.sx,
                self.sy,
            )
            .filter(|&(cell, _)| self.outside_interiors(cell));
            if let Some(f) = face {
                pair[side] = Some((f, true));
                continue;
            }
            let (x0, y0, x1, y1) = self.boxes[i];
            let off = |c: &&(i64, i64)| self.outside_interiors(**c);
            let anchor = if side == 0 {
                pts.iter().find(off)
            } else {
                pts.iter().rev().find(off)
            };
            if let Some(&anchor) = anchor {
                let d = step_dir(anchor, ((x0 + x1) / 2, (y0 + y1) / 2));
                if d != 0 {
                    pair[side] = Some(((anchor, d), false));
                }
            }
        }
        pair
    }
    /// A plain end on a face: the border opens into the line.
    fn junction(&self, cv: &mut Canvas, cell: (i64, i64), dir: u8) {
        if matches!(cv.symbol(cell.0, cell.1), Some("─" | "│")) {
            cv.swap(cell.0, cell.1, face_junction(dir));
        }
    }
    /// Write `s` at the first candidate whose cells are all blank, else
    /// the first clear of every box; nowhere when neither is.
    fn place(&self, cv: &mut Canvas, cands: &[(i64, i64)], s: &str, style: Style) {
        let n = cells(s);
        let free = |c: i64, r: i64| (0..n).all(|k| cv.blank(c + k, r));
        let clear = |c: i64, r: i64| (0..n).all(|k| !self.in_box((c + k, r)));
        let spot = cands
            .iter()
            .find(|&&(c, r)| free(c, r))
            .or_else(|| cands.iter().find(|&&(c, r)| clear(c, r)))
            .copied();
        if let Some((c, r)) = spot {
            cv.text(c, r, s, style);
        }
    }
    /// Every edge label beside the line cell nearest the engine's spot:
    /// right/left of a vertical stretch, above/below a horizontal one;
    /// the engine's spot itself as the last resort.
    fn labels(&self, cv: &mut Canvas, focus: Option<&str>, color: Color) {
        for (e, path) in self.sc.edges.iter().zip(&self.paths) {
            let Some((text, (x, y), _)) = &e.label else {
                continue;
            };
            let text = flat_label(text);
            if text.is_empty() {
                continue;
            }
            let n = cells(&text);
            let faded = focus.is_some() && !edge_touches(e, focus);
            let style = styled(if faded { Color::DarkGray } else { color }, false);
            let (lc, lr) = (self.cx(*x), self.cy(*y));
            let near = path
                .iter()
                .filter(|&&c| !self.in_box(c))
                .min_by_key(|&&(c, r)| (c - lc).abs() + (r - lr).abs())
                .copied();
            let mut cands = Vec::new();
            if let Some((c, r)) = near {
                let vertical = matches!(cv.symbol(c, r), Some("│" | "╎"));
                let beside = [(c + 2, r), (c - 1 - n, r)];
                let over = [(c - n / 2, r - 1), (c - n / 2, r + 1)];
                if vertical {
                    cands.extend(beside.into_iter().chain(over));
                } else {
                    cands.extend(over.into_iter().chain(beside));
                }
            }
            cands.push((lc - n / 2, lr));
            // Spots with a blank cell either side first, so a label
            // never reads as part of a line it sits beside.
            let (mut cands, tight): (Vec<_>, Vec<_>) = cands
                .into_iter()
                .partition(|&(c, r)| cv.blank(c - 1, r) && cv.blank(c + n, r));
            cands.extend(tight);
            self.place(cv, &cands, &text, style);
        }
    }
}

/// Where an end's `n`-cell text may go, just outside the box beside the
/// end cell `(c, r)` entered in `dir`: left of / above the line first
/// when `before`, else right of / below it.
fn card_cands((c, r): (i64, i64), dir: u8, n: i64, before: bool) -> Vec<(i64, i64)> {
    let (a, b) = match dir {
        DN => ((c - 1 - n, r - 1), (c + 2, r - 1)),
        UP => ((c - 1 - n, r + 1), (c + 2, r + 1)),
        RT => ((c - n, r - 1), (c - n, r + 1)),
        _ => ((c + 1, r - 1), (c + 1, r + 1)),
    };
    if before {
        vec![a, b]
    } else {
        vec![b, a]
    }
}

/// Draw a class diagram: the generic boxes and lines, then the class
/// overlay — compartments, UML end glyphs, cardinalities, relation
/// labels, dashed lines and notes. `opts` should come from
/// [`class_opts`] (as the app's do).
pub fn draw_class(buf: &mut Buffer, area: Rect, cs: &ClassScene, opts: &Opts) {
    draw_lines(buf, area, &cs.scene, opts, false);
    draw_class_overlay(buf, area, cs, opts);
}

fn draw_class_overlay(buf: &mut Buffer, area: Rect, cs: &ClassScene, opts: &Opts) {
    use flowmaid::model::EdgeKind;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let sc = &cs.scene;
    let geo = Geo::new(sc, area, opts);
    let (sx, sy) = (geo.sx, geo.sy);
    let cx = |x: f64| geo.cx(x);
    let cy = |y: f64| geo.cy(y);
    let boxes = &geo.boxes;
    let mut cv = Canvas { buf, area };
    let focus = opts.highlight.as_deref();
    let faded_node = |i: usize| focus.is_some_and(|h| h != sc.nodes[i].id);

    // 1) Dashed lines: realization / dependency, and note connectors.
    geo.dash(&mut cv, |e| {
        matches!(e.kind, EdgeKind::Dotted | EdgeKind::DottedOpen)
    });

    // 2) Class boxes: compartments, or just the name for a lollipop.
    for (i, (b, n)) in cs.boxes.iter().zip(&sc.nodes).enumerate() {
        let (x0, y0, x1, y1) = boxes[i];
        let faded = faded_node(i);
        let tint = |c: Color| if faded { Color::DarkGray } else { c };
        let bold = !faded && focus.is_some();
        let name_style = styled(tint(label_color(n)), !faded);
        if b.lollipop {
            for r in y0..=y1 {
                for c in x0..=x1 {
                    cv.put(c, r, ' ', Style::default());
                }
            }
            // Cut to its rect, which `class_opts` sizes to fit the name.
            let name = fit(&class_name(b), x1 - x0 + 1);
            let start = x0 + (x1 - x0 + 1 - cells(&name)).max(0) / 2;
            cv.text(start, cy(n.y), &name, name_style);
            continue;
        }
        let border = styled(tint(node_color(n)), bold);
        for r in (y0 + 1)..y1 {
            for c in (x0 + 1)..x1 {
                cv.put(c, r, ' ', Style::default());
            }
        }
        let inner = x1 - x0 - 1;
        let room = (y1 - y0 - 1).max(0) as usize;
        let lines = class_lines_within(b, room);
        let centred = |cv: &mut Canvas, r: i64, s: &str, style: Style| {
            let s = fit(s, inner);
            let start = x0 + 1 + (inner - cells(&s)).max(0) / 2;
            cv.text(start, r, &s, style);
        };
        // A lone name sits on the middle row, as `draw` puts labels.
        let top = if lines.len() == 1 {
            (y0 + y1) / 2
        } else {
            y0 + 1
        };
        for (k, l) in lines.iter().enumerate() {
            let r = top + k as i64;
            match l {
                ClassLine::Annotation(a) => {
                    centred(&mut cv, r, a, styled(tint(CLASS_ANNOTATION), false))
                }
                ClassLine::Name => centred(&mut cv, r, &class_name(b), name_style),
                ClassLine::Divider => {
                    cv.put(x0, r, '├', border);
                    for c in (x0 + 1)..x1 {
                        cv.put(c, r, '─', border);
                    }
                    cv.put(x1, r, '┤', border);
                }
                ClassLine::Member(row) => {
                    let mut style = styled(tint(CLASS_MEMBER), false);
                    if row.is_static {
                        style = style.add_modifier(Modifier::UNDERLINED);
                    }
                    if row.is_abstract {
                        style = style.add_modifier(Modifier::ITALIC);
                    }
                    cv.text(x0 + 2, r, &fit(&row.text, inner - 2), style);
                }
                ClassLine::More => centred(&mut cv, r, "…", styled(tint(CLASS_MEMBER), false)),
            }
        }
    }

    // 3) Notes: a yellow card with its lines left-aligned.
    for (k, nb) in cs.notes.iter().enumerate() {
        let i = cs.boxes.len() + k;
        let Some(&(x0, y0, x1, y1)) = boxes.get(i) else {
            continue;
        };
        let faded = faded_node(i);
        let col = if faded { Color::DarkGray } else { CLASS_NOTE };
        let style = styled(col, false);
        let bold = !faded && focus.is_some();
        let edge = styled(col, bold);
        for c in (x0 + 1)..x1 {
            cv.put(c, y0, '─', edge);
            cv.put(c, y1, '─', edge);
        }
        for r in (y0 + 1)..y1 {
            cv.put(x0, r, '│', edge);
            cv.put(x1, r, '│', edge);
            for c in (x0 + 1)..x1 {
                cv.put(c, r, ' ', Style::default());
            }
        }
        for (c, r, ch) in [(x0, y0, '┌'), (x1, y0, '┐'), (x0, y1, '└'), (x1, y1, '┘')] {
            cv.put(c, r, ch, edge);
        }
        let inner = x1 - x0 - 1;
        let lines: Vec<String> = nb.text.split('\n').map(flat_label).collect();
        if lines.len() as i64 > y1 - y0 - 1 {
            cv.text(
                x0 + 2,
                (y0 + y1) / 2,
                &fit(&lines.join(" "), inner - 2),
                style,
            );
        } else {
            for (k, l) in lines.iter().enumerate() {
                cv.text(x0 + 2, y0 + 1 + k as i64, &fit(l, inner - 2), style);
            }
        }
    }

    // 4) End glyphs (or a junction where a line meets a face plainly),
    // remembered per end for the cardinalities.
    let mut ends: Vec<[Option<FaceEnd>; 2]> = Vec::with_capacity(sc.edges.len());
    for (j, e) in sc.edges.iter().enumerate() {
        let rel = cs.rels.get(j);
        let faded = focus.is_some() && !edge_touches(e, focus);
        let line = if faded {
            Color::DarkGray
        } else {
            geo.line_color(e)
        };
        let pair = geo.ends(j);
        for (side, end) in pair.iter().enumerate() {
            let Some(((cell, dir), on_face)) = *end else {
                continue;
            };
            let kind = rel.and_then(|r| if side == 0 { r.from_kind } else { Some(r.kind) });
            match kind.and_then(|k| rel_glyph(k, dir)) {
                Some(g) => cv.put(cell.0, cell.1, g, styled(line, !faded)),
                None if on_face => geo.junction(&mut cv, cell, dir),
                None => {}
            }
        }
        ends.push(pair.map(|p| p.map(|(f, _)| f)));
    }

    // 5) Cardinalities beside their end, just outside the box, on the
    // side the engine put them; then relation labels beside the line.
    let spots = flowmaid::class::card_spots(cs);
    for (j, rel) in cs.rels.iter().enumerate() {
        let e = &sc.edges[j];
        let faded = focus.is_some() && !edge_touches(e, focus);
        let style = styled(if faded { Color::DarkGray } else { CLASS_CARD }, false);
        for (side, card) in [&rel.from_card, &rel.to_card].into_iter().enumerate() {
            let Some(card) = card.as_deref().map(flat_label) else {
                continue;
            };
            let n = cells(&card);
            let spot = spots.get(j).and_then(|s| s[side]);
            let cands = match ends.get(j).and_then(|p| p[side]) {
                Some(((c, r), dir)) => {
                    // `before`: the engine put the card left of (above) the end.
                    let before = spot.is_some_and(|(x, y)| match dir {
                        DN | UP => x < c as f64 * sx,
                        _ => y < r as f64 * sy,
                    });
                    card_cands((c, r), dir, n, before)
                }
                None => spot
                    .map(|(x, y)| vec![(cx(x) - n / 2, cy(y))])
                    .unwrap_or_default(),
            };
            geo.place(&mut cv, &cands, &card, style);
        }
    }
    geo.labels(&mut cv, focus, CLASS_REL_LABEL);
}

/// Headless render of a class diagram, overlay included. `opts` should
/// come from [`class_opts`] (as the app's do).
pub fn preview_class(cs: &ClassScene, cols: u16, rows: u16, opts: &Opts) -> String {
    render_text(cols, rows, |buf, area| draw_class(buf, area, cs, opts))
}

// ---------------------------------------------------------------------
// ER diagrams
// ---------------------------------------------------------------------

/// Colours of the ER overlay's text. Table borders and lines keep the
/// generic painter's colours.
const ER_TYPE: Color = Color::Gray;
const ER_NAME: Color = Color::White;
const ER_KEYS: Color = Color::Yellow;
const ER_COMMENT: Color = Color::DarkGray;
const ER_CARD: Color = Color::LightBlue;
const ER_REL_LABEL: Color = Color::White;
/// Blank cells between a table's type / name / keys / comment columns.
const ER_COL_GAP_CELLS: i64 = 2;

/// One cell row of an entity table, top to bottom.
enum ErLine<'a> {
    Title,
    Divider,
    Row(&'a ErRow),
    /// Rows cut for lack of room.
    More,
}

/// Every line of a table: its title, then (when it has columns) a
/// divider and one line per column.
fn er_lines(t: &ErTable) -> Vec<ErLine<'_>> {
    let mut out = vec![ErLine::Title];
    if !t.rows.is_empty() {
        out.push(ErLine::Divider);
        out.extend(t.rows.iter().map(ErLine::Row));
    }
    out
}

/// The lines that fit `room` inner rows: all of them, else the title,
/// divider and as many columns as fit before a `…` line; the title alone
/// (with `…` under it when there is a second row) when that is all.
fn er_lines_within(t: &ErTable, room: usize) -> Vec<ErLine<'_>> {
    let mut lines = er_lines(t);
    if lines.len() <= room {
        return lines;
    }
    if room <= 2 {
        lines.truncate(1);
        if room == 2 {
            lines.push(ErLine::More);
        }
        return lines;
    }
    lines.truncate(room - 1);
    lines.push(ErLine::More);
    lines
}

/// Column widths of a table, in cells (0 for a column no row uses).
struct ErCols {
    ty: i64,
    name: i64,
    keys: i64,
    comment: i64,
}

fn er_cols(t: &ErTable) -> ErCols {
    let widest = |f: fn(&ErRow) -> &str| t.rows.iter().map(|r| cells(f(r))).max().unwrap_or(0);
    ErCols {
        ty: widest(|r| &r.ty),
        name: widest(|r| &r.name),
        keys: widest(|r| &r.keys),
        comment: widest(|r| &r.comment),
    }
}

impl ErCols {
    /// Where each column starts, from the row's first cell; keys end
    /// (right-aligned) at `keys + self.keys`.
    fn starts(&self) -> (i64, i64, i64) {
        let gap = ER_COL_GAP_CELLS;
        let name = self.ty + gap;
        let keys = name + self.name + gap;
        let comment = if self.keys > 0 {
            keys + self.keys + gap
        } else {
            keys
        };
        (name, keys, comment)
    }
    /// A whole row's width.
    fn width(&self) -> i64 {
        let (_, keys, comment) = self.starts();
        if self.comment > 0 {
            comment + self.comment
        } else if self.keys > 0 {
            keys + self.keys
        } else {
            keys - ER_COL_GAP_CELLS
        }
    }
}

/// How a cardinality reads at its end of a relationship.
fn card_text(c: Card) -> &'static str {
    match c {
        Card::One => "1",
        Card::ZeroOne => "0..1",
        Card::ZeroMany => "0..*",
        Card::OneMany => "1..*",
    }
}

/// Options for drawing an ER diagram. The engine sizes a table by its
/// proportional text width and 22 px per column row, finer than a
/// terminal cell at the default scale, so columns would be cut or not
/// drawn at all. Picks the coarsest scale (never coarser than `base`)
/// at which every table gives each of its lines one cell row and its
/// widest line a padded fit. Compute once per scene and reuse it for
/// draw and picking.
pub fn er_opts(es: &ErScene, base: &Opts) -> Opts {
    let (mut sx, mut sy) = (base.sx, base.sy);
    for (t, n) in es.tables.iter().zip(&es.scene.nodes) {
        let widest = cells(&t.title).max(er_cols(t).width());
        if n.w > 0.0 && widest > 0 {
            sx = sx.min(n.w / (widest as f64 + BOX_TEXT_PAD_CELLS + BOX_FRAME_CELLS));
        }
        if n.h > 0.0 {
            sy = sy.min(n.h / (er_lines(t).len() as f64 + BOX_FRAME_CELLS));
        }
    }
    Opts {
        sx,
        sy,
        ..base.clone()
    }
}

/// Draw an ER diagram: the generic boxes and lines, then the ER overlay
/// — table titles and aligned columns, dashed non-identifying
/// relationships, cardinalities at both ends and relationship labels.
/// `opts` should come from [`er_opts`] (as the app's do).
pub fn draw_er(buf: &mut Buffer, area: Rect, es: &ErScene, opts: &Opts) {
    draw_lines(buf, area, &es.scene, opts, false);
    draw_er_overlay(buf, area, es, opts);
}

fn draw_er_overlay(buf: &mut Buffer, area: Rect, es: &ErScene, opts: &Opts) {
    use flowmaid::model::EdgeKind;
    if area.width == 0 || area.height == 0 {
        return;
    }
    let sc = &es.scene;
    let geo = Geo::new(sc, area, opts);
    let mut cv = Canvas { buf, area };
    let focus = opts.highlight.as_deref();

    // 1) Non-identifying relationships are dashed.
    geo.dash(&mut cv, |e| matches!(e.kind, EdgeKind::Dotted));

    // 2) Tables: the title, a divider, then the columns aligned.
    for (i, (t, n)) in es.tables.iter().zip(&sc.nodes).enumerate() {
        let Some(&(x0, y0, x1, y1)) = geo.boxes.get(i) else {
            continue;
        };
        let faded = focus.is_some_and(|h| h != n.id);
        let tint = |c: Color| if faded { Color::DarkGray } else { c };
        let border = styled(tint(node_color(n)), !faded && focus.is_some());
        for r in (y0 + 1)..y1 {
            for c in (x0 + 1)..x1 {
                cv.put(c, r, ' ', Style::default());
            }
        }
        let inner = x1 - x0 - 1;
        let lines = er_lines_within(t, (y1 - y0 - 1).max(0) as usize);
        let centred = |cv: &mut Canvas, r: i64, s: &str, style: Style| {
            let s = fit(s, inner);
            cv.text(x0 + 1 + (inner - cells(&s)).max(0) / 2, r, &s, style);
        };
        // A lone title sits on the middle row, as `draw` puts labels.
        let top = if lines.len() == 1 {
            (y0 + y1) / 2
        } else {
            y0 + 1
        };
        let cols = er_cols(t);
        let (name_at, keys_at, comment_at) = cols.starts();
        // Text runs from one cell in to one cell short of the border.
        let (left, limit) = (x0 + 2, x1 - 1);
        let seg = |cv: &mut Canvas, c: i64, r: i64, s: &str, style: Style| {
            if !s.is_empty() && c < limit {
                cv.text(c, r, &fit(s, limit - c), style);
            }
        };
        for (k, l) in lines.iter().enumerate() {
            let r = top + k as i64;
            match l {
                ErLine::Title => {
                    centred(&mut cv, r, &t.title, styled(tint(label_color(n)), !faded))
                }
                ErLine::Divider => {
                    cv.put(x0, r, '├', border);
                    for c in (x0 + 1)..x1 {
                        cv.put(c, r, '─', border);
                    }
                    cv.put(x1, r, '┤', border);
                }
                ErLine::Row(row) => {
                    seg(&mut cv, left, r, &row.ty, styled(tint(ER_TYPE), false));
                    seg(
                        &mut cv,
                        left + name_at,
                        r,
                        &row.name,
                        styled(tint(ER_NAME), false),
                    );
                    let keys_from = left + keys_at + cols.keys - cells(&row.keys);
                    seg(
                        &mut cv,
                        keys_from,
                        r,
                        &row.keys,
                        styled(tint(ER_KEYS), !faded),
                    );
                    let comment = styled(tint(ER_COMMENT), false).add_modifier(Modifier::ITALIC);
                    seg(&mut cv, left + comment_at, r, &row.comment, comment);
                }
                ErLine::More => centred(&mut cv, r, "…", styled(tint(ER_TYPE), false)),
            }
        }
    }

    // 3) Each end: the border opens into the line, its cardinality
    // beside it just outside the table; then the relationship labels.
    for (j, e) in sc.edges.iter().enumerate() {
        let faded = focus.is_some() && !edge_touches(e, focus);
        let style = styled(if faded { Color::DarkGray } else { ER_CARD }, false);
        let pair = geo.ends(j);
        let cards = es.cards.get(j).map(|&(a, b)| [a, b]);
        for (side, end) in pair.iter().enumerate() {
            let Some(((cell, dir), on_face)) = *end else {
                continue;
            };
            if on_face {
                geo.junction(&mut cv, cell, dir);
            }
            if let Some(card) = cards.map(|c| card_text(c[side])) {
                let n = cells(card);
                let mut cands = card_cands(cell, dir, n, false);
                // Beside a vertical line: spots with a blank cell either
                // side first (each also tried one cell further from the
                // line), so a card never reads as part of the line. Above
                // or below a horizontal one the row itself keeps it clear.
                if matches!(dir, DN | UP) {
                    let away = |(c, r): (i64, i64)| (c + (c - cell.0).signum(), r);
                    let (padded, tight): (Vec<_>, Vec<_>) = cands
                        .iter()
                        .flat_map(|&p| [p, away(p)])
                        .partition(|&(c, r)| cv.blank(c - 1, r) && cv.blank(c + n, r));
                    cands = padded.into_iter().chain(tight).collect();
                }
                geo.place(&mut cv, &cands, card, style);
            }
        }
    }
    geo.labels(&mut cv, focus, ER_REL_LABEL);
}

/// Headless render of an ER diagram, overlay included. `opts` should
/// come from [`er_opts`] (as the app's do).
pub fn preview_er(es: &ErScene, cols: u16, rows: u16, opts: &Opts) -> String {
    render_text(cols, rows, |buf, area| draw_er(buf, area, es, opts))
}
