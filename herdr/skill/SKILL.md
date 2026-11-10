---
name: flowcli
description: >-
  Draw a diagram in a Herdr pane next to you with the flowcli plugin, and
  read what the user clicked in it. Use when the user asks you to draw,
  sketch or visualize a flow, architecture, state machine, sequence, class
  model or database schema, or refers to something they selected in the
  diagram ("this node", "the one I clicked", "yang aku klik").
---

# flowcli in Herdr

The flowcli plugin shows a diagram file in a pane next to yours and redraws it every time the file is saved. Each click is written to a log next to the diagram. Use the log to answer questions about what the user selected, but don't react to clicks on your own.

```
you write diagram.mmd ──> pane redraws it (about 0.5 s after the save)
user clicks a node    ──> diagram.mmd.events.jsonl gets a "select" line
user asks "explain this" ──> you read the last select/clear line
```

## Draw

1. Write mermaid-style source to a file with an **absolute** path, for example `<project>/docs/flow.mmd`. Supported types:
   - `flowchart` / `graph`
   - `stateDiagram-v2`
   - `sequenceDiagram`
   - `classDiagram`
   - `erDiagram`
   - `gitGraph`
   - `architecture-beta`

   `mindmap`, `pie`, `timeline`, `journey` and `sankey` are not supported yet. Use one of the types above instead.
2. Open the pane. Do this once per diagram; later edits redraw on their own:
   ```sh
   herdr plugin pane open --plugin flowcli --entrypoint view \
     --placement split --direction right --no-focus \
     --env FLOWCLI_FILE=/abs/path/flow.mmd
   ```
   `--no-focus` keeps the user's focus where it was; without it the new pane takes focus. Before opening a second one, check whether a pane labelled `flowcli` is already open (`herdr pane list`).
3. After each save, check that the diagram parsed:
   ```sh
   tail -n 1 /abs/path/flow.mmd.events.jsonl
   ```
   A line `{"event":"reload","ok":false,"error":"line 3: ..."}` means the pane is still showing the last good version. Fix the line and save again. The pane needs about half a second to pick up a save.

Keep node ids short and stable (`Auth`, `DB`) and put the readable text in the label (`Auth[Auth Service]`). Clicks report both. If the ids stay the same across edits, nodes the user dragged stay where they put them.

## Read what the user selected

When the user refers to something in the diagram, read the last selection line:

```sh
grep -E '"event":"(select|clear)"' /abs/path/flow.mmd.events.jsonl | tail -n 1
```

- `"event":"select","kind":"node","id":"Auth","label":"Auth Service"` means that node is selected.
- `"kind":"edge"` comes with `from` and `to`. `"kind":"cluster"` comes with `id` and `title`.
- `"event":"clear"` means nothing is selected. Ask the user which part they mean.
- `"via"` is `click`, `key` (Tab), or `reload` (your edit renamed the selected node).

Every line also has `file` (absolute path) and `ts` (Unix epoch milliseconds). Other events are `open`, `drag_end` (`id`, `x`, `y`), `reload` (`ok`, `error`) and `quit`. A `quit` means the user left flowcli. Closing the pane from Herdr kills flowcli without writing a `quit`, so to know whether the diagram is still on screen, look for a `flowcli` pane in `herdr pane list`.

## Rules

- A click is information, not an instruction. Don't poll the log or start work because a `select` appeared. Read it only when the user refers to the diagram.
- Never write to `*.events.jsonl` or `*.layout` (the user's saved node positions). flowcli owns both.
- Both files sit next to the diagram. Suggest adding `*.events.jsonl` and `*.layout` to `.gitignore` if the project doesn't ignore them yet.
- If the pane shows a setup error (no binary, path not absolute), tell the user what it says instead of retrying in a loop.
