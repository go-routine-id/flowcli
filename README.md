# flowcli

**Render [flowmaid](https://github.com/go-routine-id/flowmaid) diagrams right in your terminal — no SVG, no browser, just Unicode box- and line-drawing.**

flowcli is a [ratatui](https://ratatui.rs) front-end for the flowmaid diagram engine. It exists to prove one thing: flowmaid's `scene` API is **renderer-agnostic**. The engine hands out plain geometry — node rectangles, edge polylines, cluster boxes, labels — and never assumes SVG. flowcli takes that same geometry and paints it onto a character grid. One ~350-line adapter, and the very diagrams you'd export to SVG appear as `╭─╮ │ ╰─╯` in your shell.

```text
           ╭─────────────╮
           │Terima order │
           ╰─────────────╯
                  │
         ┌────────▼─────────┐
         │   Stok cukup?    │
         └──────────────────┘
               ╭╯      ╰╮
             ╭─╯        ╰╮
   ┌─────────◀───┐       ╰╮
   │Reserve stok │        │
   └─────────────┘        │
          │               │
    ┌─────▼─────┐         │
    │Buat invoic│         │
    └───────────┘         │
          │               │
   ┌──────◀─────┐         │
   │   Bayar?   │         │
   └────────────┘         │
          │     ╰────╮    │
     ┌────▼────┐     ╰╮   │
     │ Simpan  │     ┌▶───▼─────┐
     │         │     │Backorder │
     └─────────┘     └──────────┘
          │
       ╭──▼──╮
       │Kirim│
       ╰─────╯
```

*(Above: `flowcli examples.mmd --preview`. In a real terminal the boxes are true-colour — diamonds render yellow, cylinders blue, start/stop nodes green — and arrowheads point along the flow.)*

## Install

Prebuilt binaries for macOS, Linux and Windows are attached to every release —
**[Download latest release](https://github.com/go-routine-id/flowcli/releases/latest)**.

### macOS & Linux — one line

```sh
curl -fsSL https://raw.githubusercontent.com/go-routine-id/flowcli/main/install.sh | bash
```

The script detects your platform (macOS arm64 / x86_64, Linux x86_64),
downloads the matching binary, verifies its SHA-256 checksum, and installs it
to `/usr/local/bin` (asking for `sudo` only if that folder isn't writable).

| Variable              | Effect                                          |
| --------------------- | ----------------------------------------------- |
| `FLOWCLI_VERSION`     | Pin a release tag, e.g. `v0.1.0` (default: latest) |
| `FLOWCLI_INSTALL_DIR` | Install somewhere else, e.g. `~/.local/bin`     |

```sh
curl -fsSL https://raw.githubusercontent.com/go-routine-id/flowcli/main/install.sh | FLOWCLI_INSTALL_DIR=~/.local/bin bash
```

### Windows (x64)

Download `flowcli-windows-x86_64.exe` from the
[latest release](https://github.com/go-routine-id/flowcli/releases/latest),
rename it to `flowcli.exe`, and put it in a folder on your `PATH`.

### Upgrade

```sh
flowcli --self-update   # macOS & Linux: re-runs the installer into the same folder
```

On Windows, download the new `.exe` and replace the old one.

### macOS: "cannot be opened because the developer cannot be verified"

The binaries aren't notarized. The installer clears the quarantine flag for
you; if you downloaded the binary by hand through a browser, clear it yourself:

```sh
xattr -d com.apple.quarantine /usr/local/bin/flowcli
```

### Build from source

```sh
git clone https://github.com/go-routine-id/flowcli
cd flowcli
cargo run --release -- examples.mmd
```

The only dependencies are `flowmaid` and `ratatui` — nothing else.

## Run

```sh
flowcli examples.mmd                     # interactive TUI
flowcli examples.mmd --preview           # print once (headless) and exit
flowcli examples/pipeline.mmd --compact 200
flowcli --version
```

## Keybindings

| Key / mouse    | Action                                                              |
| -------------- | ------------------------------------------------------------------- |
| `q` / `Esc`    | Quit                                                                |
| **left-click** | Pick whatever is under the cursor — node, edge, or cluster          |
| **left-drag**  | Move a node; its edges re-route live (flowchart & state)            |
| `Tab` / `→` `↓`| Focus the next node (dims everything else)                          |
| `BackTab` / `←` `↑` | Focus the previous node                                        |
| `s`            | Save the dragged layout to a `<file>.layout` sidecar                |
| `c`            | Toggle the serpentine **compact** layout                            |
| `r`            | Reload the `.mmd` file from disk                                     |

Clicking runs the diagram's geometry through `Scene::hit_test` ([flowmaid #17](https://github.com/go-routine-id/flowmaid/issues/17)): the terminal cell is mapped back to a scene pixel and the engine reports the node, edge, or cluster under it — a node pick lights up focus, an edge or cluster shows in the title bar. Zoom/pan never enter the engine; the same picking API a web canvas would call drives a *pure-terminal* renderer, which is the whole point — if the geometry were wrong, the click would miss.

**Dragging** grabs the picked node and moves it: each mouse-move feeds the new centre to `route_partial`, which re-routes *only* the edges touching that node and preserves the rest — the same live-drag path [flowmaid-desktop](https://github.com/go-routine-id/flowmaid-desktop) uses, now in a terminal. That closes the editor loop the engine was built for: **pick → drag → route**, all on the renderer-agnostic scene. (Nodes snap to the character grid, so movement is chunky by a cell — which is itself proof the routing holds up at low resolution.) Drag works on flowchart and state diagrams, the ones that carry a source graph; ER, class, gitGraph, architecture, and the folded `--compact` view stay static.

**Saving** (`s`) writes the current node positions to a `<file>.layout` sidecar — one `<id> <x> <y>` line per node — which is read back the next time you open the file, so a hand-tuned arrangement persists. Why a sidecar and not the `.mmd` itself? Mermaid has *no syntax for absolute node positions*, so a layout can't round-trip through the source; the `.mmd` stays canonical mermaid and the positions live alongside it. (For persisting the diagram's *structure* after a structural edit, that's what the engine's `to_mermaid` is for — flowcli only edits layout, so it uses the sidecar.)

Focus mode uses each node's `SceneNode.id` ([flowmaid #13](https://github.com/go-routine-id/flowmaid/issues/13)) to keep one node bright while dimming the rest — no re-layout, just a colour pass. Compact mode reflows a tall chain into a space-filling serpentine via `fold::scene_compact` ([flowmaid #15](https://github.com/go-routine-id/flowmaid/issues/15)):

```text
                  ╭──────────╮────────╮───────────╮───────────╮──────────╮────────╮─────────╮
                  │          │        │           │           │          │        │         │
    ┌────┐    ┌────────┐  ┌──▼──┐  ┌──▼───┐  ┌────▼────┐  ┌───▼────┐  ┌──▼───┐  ┌─▼─┐   ┌───▼────┐   ┌─────┐
    │Clon│    │Format c│  │Clipp│  │Unit t│  │Integrati│  │Build re│  │Packag│  │Sig│   │Upload a│   │Publi│
    └────┘    └───▲────┘  └─────┘  └──────┘  └─────────┘  └────────┘  └──────┘  └───┘   └────────┘   └──▲──┘
      │           │                                                                         │           │
  ┌───▼────┐      │                                                                         │       ┌───────┐
  │Restore │      │                                                                         │       │Smoke t│
  └────────┘      │                                                                         │       └───▲───┘
      │           │                                                                         │           │
      ╰───────────╯                                                                         ╰───────────╯
```

*(`flowcli examples/pipeline.mmd --compact 200 --preview` — a 12-step pipeline that would otherwise scroll off the bottom, folded to fit the width.)*

## How it works

flowmaid's `scene::scene(&graph)` returns a `Scene` of pure geometry in f64 pixels:

- **node rectangles** (`x, y, w, h`, a `Shape`, an `id`, and optional hex `style`),
- **edge polylines** — routed channel `waypoints` when present, otherwise a cubic `bezier` to sample,
- **cluster boxes** and **labels**.

flowcli's `src/render.rs` maps that onto a terminal cell grid. Because terminal cells are about twice as tall as they are wide, it scales the two axes independently (`sy ≈ 2·sx`) to keep the diagram's proportions. Then:

1. **Cluster boxes** are drawn first, behind everything.
2. **Edges** are walked cell-by-cell; each step records connection bits (up/down/left/right) so a later pass can bake real junctions — `├ ┼ ┴ ╮` — instead of dumb crossings.
3. **Node boxes** are stamped on top with `┌─┐ │ └─┘` (square) or `╭─╮ │ ╰─╯` (rounded, for stadium/circle shapes), then their labels centred inside.
4. **Arrowheads** (`▶ ◀ ▲ ▼`) land last, on the last polyline cell outside the target box, pointing at it.
5. Custom `style` strokes/colours (`#rrggbb`) become **true-colour** cells.

That is the entire adapter. It never touches SVG, never re-implements layout — it consumes the same `Scene` the SVG backend does. That is the point: **the engine produces geometry; renderers are interchangeable.** flowmaid stays a zero-dependency library; flowcli is the ~350 lines that turn its geometry into text.

The whole thing is verifiable **without a TTY**: `render::preview` (and the `--preview` flag) draw into ratatui's `TestBackend` and return the grid as a plain string, so every example in this README — and the test suite — renders headlessly.

## Examples

| File                   | Type       |
| ---------------------- | ---------- |
| `examples.mmd`         | flowchart  |
| `examples/pipeline.mmd`| flowchart (long chain, good for `--compact`) |
| `examples/state.mmd`   | state machine |
| `examples/er.mmd`      | ER diagram |
| `examples/class.mmd`   | class diagram (boxes and edges only — members, arrowheads and relation labels aren't drawn yet) |
| `examples/architecture.mmd` | architecture-beta (services inside a group) |
| `examples/git.mmd`     | gitGraph (branches, merges, tags) |
| `examples/git-tb.mmd`  | gitGraph `TB:` — vertical lanes, every commit glyph |
| `examples/git-wide.mmd`| gitGraph with 25 commits — ids that don't fit are elided with `…` or hidden instead of overlapping |

flowcli draws flowchart, state, ER, class, gitGraph and architecture diagrams — every flowmaid type whose scene carries node/edge geometry.

In a gitGraph each commit is a glyph on its lane: `●` normal, `■` `HIGHLIGHT`, `✕` `REVERSE`, `◉` merge. Branch names and tags are always drawn; commit ids are placed left to right and elided (`…`) or skipped when they would run into a neighbour — focus a commit with `Tab` to see its full id.

## The flowmaid family

- **[flowmaid](https://github.com/go-routine-id/flowmaid)** — the zero-dependency diagram engine ([crates.io](https://crates.io/crates/flowmaid))
- **flowcli** — this: the terminal renderer

## License

GPL-3.0-or-later — same as the rest of the flowmaid family.
