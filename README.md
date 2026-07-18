# flowcli

Render [flowmaid](https://crates.io/crates/flowmaid) diagrams **in the terminal** with [ratatui](https://ratatui.rs) — a proof that flowmaid's scene API is renderer-agnostic: no SVG, just plain geometry redrawn with Unicode box- and line-drawing characters.

```sh
flowcli diagram.mmd            # interactive
flowcli diagram.mmd --preview  # print once (headless) and exit
flowcli diagram.mmd --compact 200
```

Interactive keys: `q` quit · `Tab`/`BackTab` focus a node (dims the rest — uses `SceneNode.id`) · `c` toggle compact serpentine layout · `r` reload the file.

## How it works

flowmaid gives `scene::scene(&graph)` → node rectangles, edge polylines (channel waypoints or bezier), cluster boxes, labels — all f64 pixels. `src/render.rs` maps those onto character cells: nodes become `╭─╮│╰─╯` boxes, edges become `├┼┴` line-drawing with `▶◀▲▼` arrowheads, and custom `style` colours (hex) become true-colour cells. flowmaid stays a zero-dependency library; flowcli is just one ~350-line adapter on top.

## License

GPL-3.0-or-later — same as flowmaid.
