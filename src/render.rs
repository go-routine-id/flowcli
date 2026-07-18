//! Draw a flowmaid [`Scene`] into a terminal cell grid.
//!
//! flowmaid hands us plain geometry — node rectangles, edge polylines,
//! cluster boxes, labels, all in f64 pixels — and never assumes a
//! renderer. Here we map those pixels onto character cells and paint
//! them with Unicode box-drawing (`┌─┐│`) for nodes, line-drawing with
//! real junctions (`├┼┴╮`) for edges, and arrowheads (`▶◀▲▼`) at the
//! ends. Custom `style` colours (hex) become true-colour cells.

use flowmaid::model::Shape;
use flowmaid::scene::{Scene, SceneNode};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Rendering options. `sx`/`sy` are pixels-per-cell on each axis;
/// terminal cells are roughly twice as tall as wide, so `sy ≈ 2·sx`
/// keeps the diagram's proportions. `highlight` dims everything except
/// one node id (drives the interactive "focus a node" mode — this is
/// what `SceneNode.id` from issue #13 is for).
#[derive(Clone)]
pub struct Opts {
    pub sx: f64,
    pub sy: f64,
    pub highlight: Option<String>,
}

impl Default for Opts {
    fn default() -> Self {
        Opts {
            sx: 8.5,
            sy: 17.0,
            highlight: None,
        }
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
        let pts: Vec<(i64, i64)> = edge_points(e)
            .into_iter()
            .map(|(x, y)| (cx(x), cy(y)))
            .collect();
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
        // Arrowhead: the last polyline cell OUTSIDE the target node box,
        // pointing toward it. Placed after nodes are drawn.
        if e.kind.has_arrow() {
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
        let (x0, y0, x1, y1) = box_of[n.id.as_str()];
        let rounded = matches!(
            n.shape,
            Shape::Rounded | Shape::Stadium | Shape::Circle | Shape::DoubleCircle
        );
        rect(&mut g, x0, y0, x1, y1, col, rounded, focus);
        for r in (y0 + 1)..y1 {
            for c in (x0 + 1)..x1 {
                g.put(c, r, ' ', col, false);
            }
        }
        let text_col = if faded {
            Color::DarkGray
        } else {
            label_color(n)
        };
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

/// Map a terminal cell back to scene space and pick what's under it.
///
/// This is the inverse of `draw`'s `cx`/`cy`: a scene point `x` is drawn
/// at cell `round(x / sx)`, so the scene point at the centre of cell `c`
/// is `c * sx`. `col`/`row` are absolute terminal coordinates and `area`
/// is the inner canvas rect the scene was drawn into. Returns `None`
/// outside the canvas or on empty space. Zoom/pan never enter the crate —
/// this is the whole screen→scene bridge, and [`flowmaid::scene::Scene::hit_test`]
/// does the geometry.
pub fn pick(scene: &Scene, area: Rect, col: u16, row: u16, opts: &Opts) -> Option<flowmaid::scene::Hit> {
    if col < area.x || row < area.y {
        return None;
    }
    let (lc, lr) = (col - area.x, row - area.y);
    if lc >= area.width || lr >= area.height {
        return None;
    }
    let (sx, sy) = scale(scene, area.width, area.height, opts);
    let x = lc as f64 * sx;
    let y = lr as f64 * sy;
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
