//! Draw a flowmaid [`Scene`] into a terminal cell grid.
//!
//! flowmaid hands us plain geometry — node rectangles, edge polylines,
//! cluster boxes, labels, all in f64 pixels — and never assumes a
//! renderer. Here we map those pixels onto character cells and paint
//! them with Unicode box-drawing (`┌─┐│`) for nodes, line-drawing with
//! real junctions (`├┼┴╮`) for edges, and arrowheads (`▶◀▲▼`) at the
//! ends. Custom `style` colours (hex) become true-colour cells.

use flowmaid::gitgraph::GitScene;
use flowmaid::model::Shape;
use flowmaid::scene::{Scene, SceneNode};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

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
        let x0 = cx(n.x - n.w / 2.0);
        let mut x1 = cx(n.x + n.w / 2.0);
        let y0 = cy(n.y - n.h / 2.0);
        let mut y1 = cy(n.y + n.h / 2.0);
        if x1 - x0 < 2 {
            x1 = x0 + 2;
        }
        if y1 - y0 < 2 {
            y1 = y0 + 2;
        }
        box_of.insert(n.id.as_str(), (x0, y0, x1, y1));
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
        let entry = if e.kind.has_arrow() {
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
        } else if e.kind.has_arrow() {
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
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    term.draw(|f| {
        let area = f.area();
        draw(f.buffer_mut(), area, scene, opts);
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..rows {
        for x in 0..cols {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

/// Headless render of a git graph, including branch labels and tags.
/// `opts` should come from [`git_opts`] (as the app's do).
pub fn preview_with_git(gs: &GitScene, cols: u16, rows: u16, opts: &Opts) -> String {
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    let mut term = Terminal::new(TestBackend::new(cols, rows)).unwrap();
    term.draw(|f| {
        let area = f.area();
        draw(f.buffer_mut(), area, &gs.scene, opts);
        draw_gitgraph_labels(f.buffer_mut(), area, gs, opts);
    })
    .unwrap();
    let buf = term.backend().buffer().clone();
    let mut out = String::new();
    for y in 0..rows {
        for x in 0..cols {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}
