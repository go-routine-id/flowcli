# flowcli

**Render [flowmaid](https://github.com/go-routine-id/flowmaid) diagrams right in your terminal — no SVG, no browser, just Unicode box- and line-drawing.**

flowcli is a [ratatui](https://ratatui.rs) front-end for the flowmaid diagram engine. It exists to prove one thing: flowmaid's `scene` API is **renderer-agnostic**. The engine hands out plain geometry — node rectangles, edge polylines, cluster boxes, labels — and never assumes SVG. flowcli takes that same geometry and paints it onto a character grid. One ~350-line adapter, and the very diagrams you'd export to SVG appear as `╭─╮ │ ╰─╯` in your shell.

```text
           ╭─────────────╮
           │Terima order │
           ╰─────────────╯
                  │
                  │
         ┌────────▼─────────┐
         │                  │
         │   Stok cukup?    │
         │                  │
         └──────────────────┘
          ╭─────╯   │
          │         ╰─────╮
          │               │
   ┌──────▼──────┐        │
   │Reserve stok │        │
   │             │        │
   └─────────────┘        │
          │               │
          │               │
    ┌─────▼─────┐         │
    │Buat invoi…│         │
    └───────────┘         │
          │               │
          │               │
          │               │
   ┌──────▼─────┐         │
   │            │         │
   │   Bayar?   │         │
   │            │         │
   └────────────┘         │
          │ ╰─────────╮   │
          │           │   │
          │           │   │
     ┌────▼────┐     ┌▼───▼─────┐
     │ Simpan  │     │Backorder │
     │         │     │          │
     └─────────┘     └──────────┘
          │
          │
          │
          ▼
          ●
        Kirim
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

### As a Herdr plugin

If you run AI coding agents in [Herdr](https://github.com/herdrdev/herdr), install flowcli as a plugin instead. It brings its own binary, so nothing above is needed:

```sh
herdr plugin install go-routine-id/flowcli/herdr
```

See [Herdr plugin](#herdr-plugin-diagrams-next-to-your-ai-agent) for what it needs and how to use it.

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
flowcli diagram.mmd --watch              # redraw whenever the file changes
flowcli diagram.mmd --events picks.jsonl # log what you select as JSON Lines
flowcli --version
```

### Watching a file and following selections

`--watch` re-reads the file whenever its content changes on disk, which is what you want when an editor or an AI agent keeps rewriting it. The file is checked every 150 ms, and a change is drawn once it reads the same twice in a row, so a save written in several chunks is never parsed half-way. Nodes you dragged stay where you put them; nodes you didn't drag follow the new layout, and the focused node stays focused even if its position in the file changed. A reload never cuts a drag short: it waits until you release the node. If the new source doesn't parse, the last good diagram stays on screen and the error shows in red in the title bar until the file parses again.

Dragged nodes keep their *absolute* position, the same way the `.layout` sidecar does. If an edit shifts the rest of the layout a lot (for example, a new node inserted above), a dragged node can end up overlapping one that moved into its place. Drag it again, or press `r` to reload without your unsaved drags.

`--events <path>` appends one JSON object per line to `path` for everything you point at, so another program can follow along without scraping the screen. The file is only ever appended to, so `tail -n 1` gives the latest event. Every line has `event`, `file` (the diagram's absolute path) and `ts` (Unix epoch milliseconds):

| `event`    | Extra fields                                                                 |
| ---------- | ---------------------------------------------------------------------------- |
| `open`     | —                                                                            |
| `select`   | `kind`: `node` (`id`, `label`, `via` = `click`, `key`, or `reload` when a reload changed the selected node's label), `edge` (`from`, `to`, `label`), or `cluster` (`id`, `title`) |
| `clear`    | — nothing is selected any more: a click on empty space, or a reload removed what was selected |
| `drag_end` | `id`, `x`, `y` (the node's new centre, in scene px)                          |
| `reload`   | `ok`, plus `error` when the new source didn't parse                          |
| `quit`     | — (written when you quit; a process killed from outside, e.g. by closing its pane, can't write it) |

```jsonl
{"event":"select","file":"/work/arch.mmd","ts":1794308756256,"kind":"node","id":"A","label":"Alpha","via":"click"}
{"event":"drag_end","file":"/work/arch.mmd","ts":1794308757160,"id":"A","x":62.3,"y":132}
```

The last `select` (or a later `clear`) is always the current selection, reloads included. Both flags need the interactive view, so they can't be combined with `--preview`, and `--events` refuses to write into the diagram file itself.

### Herdr plugin: diagrams next to your AI agent

[Herdr](https://github.com/herdrdev/herdr) runs coding agents in terminal panes. The plugin in [`herdr/`](herdr/) puts the two flags above together: the agent writes a `.mmd`, a flowcli pane next to it redraws it on every save, and what you click is logged next to the diagram so the agent can read it when you say "explain this one".

```
+-------------------+  writes flow.mmd   +--------------------------+
| agent pane        |------------------->| flowcli pane             |
| (Claude, Codex,…) |                    | --watch --events         |
+-------------------+                    +------------+-------------+
          ^                                           | you click a node
          |  reads the last select/clear line         v
          +----------------------------- flow.mmd.events.jsonl
```

A click is only logged. The agent reads the log when you refer to the diagram, so clicking around never starts work on its own.

```sh
herdr plugin install go-routine-id/flowcli/herdr
```

The install downloads the prebuilt release matching the checked-out `Cargo.toml` version into the plugin's own `bin/` and verifies its checksum, so it needs no Rust toolchain and no `sudo`.

- **Needs**: Herdr 0.9.0 or newer, on macOS or Linux, with `git`, `curl` and `bash` available.
- **Per session**: Herdr keeps one plugin registry per session, so run the install in each named session where you want it.
- **Check it**: `herdr plugin list --plugin flowcli` shows the plugin, its version and where it lives.
- **Remove it**: `herdr plugin uninstall flowcli`. To move to a newer version, uninstall and install again.

- **Open a diagram** (what the agent runs; the path must be absolute):
  ```sh
  herdr plugin pane open --plugin flowcli --entrypoint view \
    --placement split --direction right --no-focus --env FLOWCLI_FILE=/abs/path/flow.mmd
  ```
  `--no-focus` leaves focus with the agent's pane. The selection log defaults to `<file>.events.jsonl`; set `FLOWCLI_EVENTS` to put it elsewhere.
- **Open the newest `.mmd` in the focused pane's folder**: the `open-diagram` action, for example `herdr plugin action invoke open-diagram --plugin flowcli`. To put it on a key, add this to Herdr's `config.toml` (pick any free key) and run `herdr server reload-config`:
  ```toml
  [[keys.command]]
  key = "prefix+alt+d"
  type = "shell"
  command = "herdr plugin action invoke open-diagram --plugin flowcli"
  ```
- **Teach your agent**: [`herdr/skill/SKILL.md`](herdr/skill/SKILL.md) tells an agent how to draw, open the pane, check that a save parsed, and read the selection. Plugin installs don't copy skills. For Claude Code, copy it yourself:
  ```sh
  mkdir -p ~/.claude/skills/flowcli
  cp "$(herdr plugin list --plugin flowcli --json | sed -n 's/.*"plugin_root":"\([^"]*\)".*/\1/p')/skill/SKILL.md" ~/.claude/skills/flowcli/
  ```

The `.events.jsonl` and `.layout` files sit next to your diagrams; add both patterns to your project's `.gitignore`.

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
| `r`            | Reload the `.mmd` file from disk, dropping unsaved drags (`--watch` reloads by itself and keeps them) |

Clicking runs the diagram's geometry through `Scene::hit_test` ([flowmaid #17](https://github.com/go-routine-id/flowmaid/issues/17)): the terminal cell is mapped back to a scene pixel and the engine reports the node, edge, or cluster under it — a node pick lights up focus, an edge or cluster shows in the title bar. Zoom/pan never enter the engine; the same picking API a web canvas would call drives a *pure-terminal* renderer, which is the whole point — if the geometry were wrong, the click would miss.

**Dragging** grabs the picked node and moves it: each mouse-move feeds the new centre to `route_partial`, which re-routes *only* the edges touching that node and preserves the rest — the same live-drag path [flowmaid-desktop](https://github.com/go-routine-id/flowmaid-desktop) uses, now in a terminal. That closes the editor loop the engine was built for: **pick → drag → route**, all on the renderer-agnostic scene. (Nodes snap to the character grid, so movement is chunky by a cell — which is itself proof the routing holds up at low resolution.) Drag works on flowchart and state diagrams, the ones that carry a source graph; ER, class, gitGraph, architecture, sequence, and the folded `--compact` view stay static.

**Saving** (`s`) writes the current node positions to a `<file>.layout` sidecar — one `<id> <x> <y>` line per node — which is read back the next time you open the file, so a hand-tuned arrangement persists. Why a sidecar and not the `.mmd` itself? Mermaid has *no syntax for absolute node positions*, so a layout can't round-trip through the source; the `.mmd` stays canonical mermaid and the positions live alongside it. (For persisting the diagram's *structure* after a structural edit, that's what the engine's `to_mermaid` is for — flowcli only edits layout, so it uses the sidecar.)

Focus mode uses each node's `SceneNode.id` ([flowmaid #13](https://github.com/go-routine-id/flowmaid/issues/13)) to keep one node bright while dimming the rest — no re-layout, just a colour pass. Compact mode reflows a tall chain into a space-filling serpentine via `fold::scene_compact` ([flowmaid #15](https://github.com/go-routine-id/flowmaid/issues/15)):

```text
                  ╭──────────╮────────╮───────────╮───────────╮──────────╮────────╮─────────╮
                  │          │        │           │           │          │        │         │
    ┌────┐    ┌────────┐  ┌──▼──┐  ┌──▼───┐  ┌────▼────┐  ┌───▼────┐  ┌──▼───┐  ┌─▼─┐   ┌───▼────┐   ┌─────┐
    │Clo…│    │Format …│  │Clip…│  │Unit …│  │Integrat…│  │Build r…│  │Packa…│  │Si…│   │Upload …│   │Publ…│
    └────┘    └───▲────┘  └─────┘  └──────┘  └─────────┘  └────────┘  └──────┘  └───┘   └────────┘   └──▲──┘
      │           │                                                                         │           │
      │           │                                                                         │           │
      │           │                                                                         │           │
  ┌───▼────┐      │                                                                         │       ┌───────┐
  │Restore…│      │                                                                         │       │Smoke …│
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

flowcli's `src/render.rs` maps that onto a terminal cell grid. Because terminal cells are about twice as tall as they are wide, it scales the two axes independently (`sy ≈ 2·sx`) to keep the diagram's proportions. A diagram larger than the terminal is scaled down to fit. Then:

1. **Cluster boxes** are drawn first, behind everything.
2. **Edges** are walked cell-by-cell; each step records connection bits (up/down/left/right) so a later pass can bake real junctions — `├ ┼ ┴ ╮` — instead of dumb crossings.
3. **Node boxes** are stamped on top with `┌─┐ │ └─┘` (square) or `╭─╮ │ ╰─╯` (rounded, for stadium/circle shapes), then every label line the box has a row for is centred inside. A line that doesn't fit its box, or the last one shown when more are left out, ends in `…`, so a cut label never passes for the whole one. Text is laid out by grapheme, so CJK characters take two cells and an emoji sequence like 👨‍👩‍👧 stays one glyph.
4. **Arrowheads** (`▶ ◀ ▲ ▼`) land last, on the last polyline cell outside the target box, pointing at it.
5. Custom `style` strokes/colours (`#rrggbb`) become **true-colour** cells.

Labels can still end in `…` at full size. The engine sizes boxes for a proportional font, about a fifth narrower than one cell per character. Laying flowcharts out with a terminal measure needs a measure-aware partial re-route in flowmaid first, so that a drag doesn't redraw edges it never touched.

That is the entire adapter. It never touches SVG, never re-implements layout — it consumes the same `Scene` the SVG backend does. That is the point: **the engine produces geometry; renderers are interchangeable.** flowmaid stays a zero-dependency library; flowcli is the ~350 lines that turn its geometry into text.

The whole thing is verifiable **without a TTY**: `render::preview` (and the `--preview` flag) draw into ratatui's `TestBackend` and return the grid as a plain string, so every example in this README — and the test suite — renders headlessly.

## Examples

| File                   | Type       |
| ---------------------- | ---------- |
| `examples.mmd`         | flowchart  |
| `examples/pipeline.mmd`| flowchart (long chain, good for `--compact`) |
| `examples/state.mmd`   | state machine |
| `examples/er.mmd`      | ER diagram — typed columns with keys and comments, an alias title, identifying and non-identifying relations, every cardinality |
| `examples/class.mmd`   | class diagram — members, `«interface»`, every relation type, cardinalities, labels, a note |
| `examples/architecture.mmd` | architecture-beta (services inside a group) |
| `examples/git.mmd`     | gitGraph (branches, merges, tags) |
| `examples/git-tb.mmd`  | gitGraph `TB:` — vertical lanes, every commit glyph |
| `examples/git-wide.mmd`| gitGraph with 25 commits — ids that don't fit are elided with `…` or hidden instead of overlapping |
| `examples/sequence.mmd`| sequence diagram — actor, every arrow type, self-message, note, activations, `alt`/`else`, `loop`, `autonumber` |

flowcli draws flowchart, state, ER, class, gitGraph, architecture and sequence diagrams.

A sequence diagram is drawn from flowmaid's own `SeqScene` (participants, lifelines, messages, notes, activations, frames) rather than a node/edge scene, so it is **static**: there is nothing to click, focus, drag or fold — only `q` and `r` apply. Participants sit in boxes at the top and again at the bottom (actors in rounded magenta boxes), lifelines are dashed `┆`, activations `┃`. A `->>` line is solid `───`, its `-->>` twin dashed `╌╌╌`; the head tells the arrow type — `▶` for `->>`, `▷` for async `-)`, `✕` for `-x`, none for a plain `->`. Labels sit above their line with the `autonumber` prefix; frames carry a `[loop]` / `[alt]` chip and `┄ [else]` dividers. A diagram wider or taller than the window is scaled down like every other type, and labels may then overlap.

```
    ╭────────╮      ┌─────────┐       ┌────────┐     ┌────────┐
    │  User  │      │   Web   │       │  API   │     │   DB   │
    ╰────┬───╯      └────┬────┘       └───┬────┘     └────┬───┘
         1. open dashboard                ┆               ┆
         ┆──────────────▶┃                ┆               ┆
         ┆               ┃   2. GET /me   ┆               ┆
         ┆               ┃───────────────▶┃               ┆
         ┆               ┃ 3. 401 expired ┃               ┆
         ┆               ┃◀╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┃               ┆
         ┆              ▌ refresh the token ▐             ┆
         ┆               ┃                ┆               ┆
   ┌[alt] token refreshed┃─────────────────────────────────────┐
   │     ┆               ┃4. POST /refresh┆               ┆    │
   │     ┆               ┃───────────────▶┆               ┆    │
   │     ┆               ┃  5. new token  ┆               ┆    │
   │     ┆               ┃◀╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┆               ┆    │
   ├[else] refresh failed┃┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┄┤
   │     6. back to login┃                ┆               ┆    │
   │     ┆✕──────────────┃                ┆               ┆    │
   └─────────────────────┃─────────────────────────────────────┘
   ┌[loop] every 30s─────┃─────────────────────────────────────┐
   │     ┆               ┃  7. heartbeat  ┆               ┆    │
   │     ┆               ┃───────────────▷┆               ┆    │
   └─────────────────────┃─────────────────────────────────────┘
         ┆               ┃                ┆───╮ 8. audit log
         ┆               ┃                ┆◀──╯           ┆
         ┆               ┃               9. write audit row
         ┆ 10. dashboard ┃                ┆───────────────┆
         ┆◀╌╌╌╌╌╌╌╌╌╌╌╌╌╌┃                ┆               ┆
    ╭────┴───╮      ┌────┴────┐       ┌───┴────┐     ┌────┴───┐
    │  User  │      │   Web   │       │  API   │     │   DB   │
    ╰────────╯      └─────────┘       └────────┘     └────────┘
```
*(`flowcli examples/sequence.mmd --preview`)*

A class diagram is drawn from flowmaid's `ClassScene`: each class box is split into compartments — stereotype and name, then attributes, then operations — with one member per row (static members underlined, abstract ones in italics). The end glyph tells the relation: `△` inheritance (solid line) or realization (dashed `╌`/`╎`), `◆` composition, `◇` aggregation, `▼` association (solid) or dependency (dashed), `○` a lollipop interface; a plain link meets the box with a junction (`┬`). Cardinalities sit just outside the box at their end, relation labels beside the line, notes in yellow cards with a dashed connector. Boxes can be focused and picked like flowchart nodes but not dragged. A diagram drawn smaller than it needs keeps every class name and cuts members first (`…`).

```
     ┌─────────────────┐       ┌────────────────────────────────┐
     │    Customer     │       │          «interface»           │       ┌───────────────────┐
     ├─────────────────┤       │            Payable             │       │ Total is cached   │
     │ +string name    │       ├────────────────────────────────┤       │ after place()     │
     │ +string email   │       │ +pay(Money amount) : bool      │       └───┬───────────────┘
     │                 │       └───────────────△────────────────┘           ╎
     └─────△─────┬─────┘                       ╎                            ╎
           │   1 │          places             ╎                            ╎
           │     ╰─────────────────────────╮   ╎    ╭╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╯
           │                               │   ╎    ╎
           │                               │ * ╎    ╎
           │                           ┌───▼───┴────┴───┐
           │                           │     Order      │
     ┌─────┴───────────┐               ├────────────────┤          ┌───────────────┐
     │   VipCustomer   │               │ +uuid id       │          │               │
     ├─────────────────┤               │ +Money total   │          │     Cart      │
     │ +int points     │               ├────────────────┤          │               │
     └─────────────────┘               │ +place()       │          └──────────◇────┘
                                       │ +cancel()      │                     │
                                       └─────┬────◆─────┘                     │
                                             ╎    │ 1                         │
                                             ╎    │           contains        │
                                    notifies ╎    ╰─────────────────────╮     │
                                             ╎                        * │     │
                                             ╎                    ┌─────┴─────┴─────┐
                                     ┌───────▼───────────┐        │    LineItem     │
                                     │      Mailer       │        ├─────────────────┤
                                     ├───────────────────┤        │ +int qty        │
                                     ├───────────────────┤        │ +Money price    │
                                     │ +send(string to)  │        │                 │
                                     └───────────────────┘        └─────────────────┘
```
*(`flowcli examples/class.mmd --preview`)*

An ER diagram is drawn from flowmaid's `ErScene`: each table shows its title (the alias when one is given, e.g. `categories["Category"]`), a divider, then one column per row with type, name, keys (`PK`, `FK`, `UK`, or a list like `PK,FK`) and comment lined up in columns. Cardinalities are written as text just outside the table at their end — `1` for `||`, `0..1` for `|o`, `0..*` for `}o`, `1..*` for `}|` — and the relation label sits beside its line. An identifying relation (`--`) is a solid line, a non-identifying one (`..`) dashed `╌`/`╎`; neither has an arrowhead. Tables can be focused and picked like flowchart nodes but not dragged. A table drawn smaller than it needs keeps its title and folds the columns that do not fit into a `…` row.

```
                     ┌────────────────────────────────────────────────┐
                     │                     users                      │
                     ├────────────────────────────────────────────────┤
                     │ uuid       id          PK                      │
                     │ varchar    email       UK  login, lower-cased  │
                     │ timestamp  created_at                          │
                     └──────────────────┬──────────┬──────────────────┘
                                       ╭╯ 1        ╰╮ 0..1
                                      ╭╯            ╎
                                    ╭─╯ writes      ╰╌╮
                                   ╭╯             has ╰╮
                                   │ 0..*              ╎
                 ┌─────────────────┴──────┐            ╰╮ 0..1
                 │         posts          │      ┌──────┴─────────────────┐
                 ├────────────────────────┤      │        profiles        │
                 │ uuid  id           PK  │      ├────────────────────────┤
                 │ uuid  author_id    FK  │      │ uuid  user_id  PK,FK   │
                 │ uuid  category_id  FK  │      │ text  bio              │
                 │ text  body             │      └────────────────────────┘
                 │                        │
                 └──────┬─────────┬───────┘
                       ╭╯ 0..*     ╎ 1..*
                     ╭─╯           ╰╌╮ tagged
                    ╭╯ in            ╰╮
                    │ 1               ╰╮ 1..*
    ┌───────────────┴────┐       ┌─────┴───────────────┐
    │      Category      │       │        tags         │
    ├────────────────────┤       ├─────────────────────┤
    │ int      id    PK  │       │ int      id    PK   │
    │ varchar  slug  UK  │       │ varchar  name       │
    └────────────────────┘       └─────────────────────┘
```
*(`flowcli examples/er.mmd --preview`)*

In a gitGraph each commit is a glyph on its lane: `●` normal, `■` `HIGHLIGHT`, `✕` `REVERSE`, `◉` merge. Branch names and tags are always drawn; commit ids are placed left to right and elided (`…`) or skipped when they would run into a neighbour — focus a commit with `Tab` to see its full id.

## The flowmaid family

- **[flowmaid](https://github.com/go-routine-id/flowmaid)** — the zero-dependency diagram engine ([crates.io](https://crates.io/crates/flowmaid))
- **flowcli** — this: the terminal renderer

## License

GPL-3.0-or-later — same as the rest of the flowmaid family.
