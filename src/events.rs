//! `--events <path>`: what the user points at, as JSON Lines.
//!
//! Every selection, drag and reload appends one JSON object (one line)
//! to the file, so another program — an AI agent, an editor plugin, a
//! script tailing the file — can follow which node or edge the user is
//! looking at without scraping the screen. The file is only ever
//! appended to: a reader can `tail -n 1` for the latest event or keep
//! its own offset.
//!
//! Every line carries `event`, `file` (the diagram's absolute path) and
//! `ts` (Unix epoch milliseconds, so readers in any timezone agree). The
//! rest depends on the event:
//!
//! | event     | extra fields                                          |
//! |-----------|-------------------------------------------------------|
//! | `open`    | —                                                     |
//! | `select`  | `kind` = `node` (`id`, `label`, `via` = click\|key\|reload), |
//! |           | `edge` (`from`, `to`, `label`), `cluster` (`id`, `title`) |
//! | `clear`   | — (nothing is selected any more: a click on empty     |
//! |           | space, or a reload removed what was selected)         |
//! | `drag_end`| `id`, `x`, `y` (node centre, scene px)                |
//! | `reload`  | `ok`, plus `error` when the new source didn't parse   |
//! | `quit`    | —                                                     |

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

/// How a node got selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    Click,
    Key,
    /// Re-reported after a reload changed the selected node's label.
    Reload,
}

/// One thing worth telling a reader about.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Open,
    SelectNode {
        id: String,
        label: String,
        via: Via,
    },
    SelectEdge {
        from: String,
        to: String,
        label: Option<String>,
    },
    SelectCluster {
        id: String,
        title: String,
    },
    Clear,
    DragEnd {
        id: String,
        x: f64,
        y: f64,
    },
    Reload {
        error: Option<String>,
    },
    Quit,
}

/// The JSON line (without the trailing newline) for `ev`.
pub fn line(ev: &Event, file: &str, ts_ms: u128) -> String {
    let name = match ev {
        Event::Open => "open",
        Event::SelectNode { .. } | Event::SelectEdge { .. } | Event::SelectCluster { .. } => {
            "select"
        }
        Event::Clear => "clear",
        Event::DragEnd { .. } => "drag_end",
        Event::Reload { .. } => "reload",
        Event::Quit => "quit",
    };
    let mut out = format!(
        "{{\"event\":{},\"file\":{},\"ts\":{ts_ms}",
        string(name),
        string(file)
    );
    let mut field = |key: &str, value: String| {
        out.push_str(&format!(",{}:{value}", string(key)));
    };
    match ev {
        Event::Open | Event::Clear | Event::Quit => {}
        Event::SelectNode { id, label, via } => {
            field("kind", string("node"));
            field("id", string(id));
            field("label", string(label));
            field(
                "via",
                string(match via {
                    Via::Click => "click",
                    Via::Key => "key",
                    Via::Reload => "reload",
                }),
            );
        }
        Event::SelectEdge { from, to, label } => {
            field("kind", string("edge"));
            field("from", string(from));
            field("to", string(to));
            field("label", label.as_deref().map_or("null".into(), string));
        }
        Event::SelectCluster { id, title } => {
            field("kind", string("cluster"));
            field("id", string(id));
            field("title", string(title));
        }
        Event::DragEnd { id, x, y } => {
            field("id", string(id));
            field("x", number(*x));
            field("y", number(*y));
        }
        Event::Reload { error } => {
            field("ok", error.is_none().to_string());
            if let Some(e) = error {
                field("error", string(e));
            }
        }
    }
    out.push('}');
    out
}

/// A JSON string literal: quotes, backslashes and control characters
/// escaped, everything else (including non-ASCII) passed through as UTF-8.
fn string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A JSON number rounded to 0.1 px; NaN/inf (never valid JSON) become null.
fn number(v: f64) -> String {
    if v.is_finite() {
        format!("{}", (v * 10.0).round() / 10.0)
    } else {
        "null".into()
    }
}

/// The open events file. Each event is a single `write_all` of one whole
/// line on an append-mode handle, so concurrent readers never see half a
/// line from us.
pub struct Sink {
    file: File,
    /// The diagram path every line reports.
    diagram: String,
    /// Set by a failed write and cleared by the next good one, so a full
    /// disk is reported once per outage instead of on every click.
    failed: bool,
}

impl Sink {
    /// Open (creating if needed) `path` for appending. `diagram` is the
    /// path written into every line — made absolute when possible, since
    /// the reader may run from another directory. Refuses to append to
    /// the diagram itself: with `--watch` every event would rewrite the
    /// file being watched, reload it, and log again — without end.
    pub fn open(path: &str, diagram: &str) -> std::io::Result<Sink> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        let same = match (std::fs::canonicalize(path), std::fs::canonicalize(diagram)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        };
        if same {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "that is the diagram itself; pick another file for events",
            ));
        }
        let diagram = std::fs::canonicalize(diagram)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| diagram.to_string());
        Ok(Sink {
            file,
            diagram,
            failed: false,
        })
    }

    /// Append `ev`. Returns a message when a write fails after one that
    /// worked (to show the user once per outage); repeats stay silent.
    pub fn emit(&mut self, ev: &Event) -> Option<String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let mut text = line(ev, &self.diagram, ts);
        text.push('\n');
        match self.file.write_all(text.as_bytes()) {
            Ok(()) => {
                self.failed = false;
                None
            }
            Err(e) if !self.failed => {
                self.failed = true;
                Some(format!("events: write failed: {e}"))
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strings_escape_quotes_backslashes_and_controls() {
        assert_eq!(string("a\"b\\c"), r#""a\"b\\c""#);
        assert_eq!(string("x\ny\tz\r"), r#""x\ny\tz\r""#);
        assert_eq!(string("\u{1}"), r#""\u0001""#);
        // Non-ASCII stays as UTF-8, it's valid inside a JSON string.
        assert_eq!(string("Käse → 表"), "\"Käse → 表\"");
    }

    #[test]
    fn numbers_round_and_never_emit_nan() {
        assert_eq!(number(12.345), "12.3");
        assert_eq!(number(-4.0), "-4");
        assert_eq!(number(f64::NAN), "null");
        assert_eq!(number(f64::INFINITY), "null");
    }

    #[test]
    fn each_event_becomes_one_line_with_its_fields() {
        let f = "/d/x.mmd";
        let sel = Event::SelectNode {
            id: "Auth".into(),
            label: "Auth \"svc\"".into(),
            via: Via::Click,
        };
        assert_eq!(
            line(&sel, f, 7),
            r#"{"event":"select","file":"/d/x.mmd","ts":7,"kind":"node","id":"Auth","label":"Auth \"svc\"","via":"click"}"#
        );
        let edge = Event::SelectEdge {
            from: "A".into(),
            to: "B".into(),
            label: None,
        };
        assert!(line(&edge, f, 0).ends_with(r#""kind":"edge","from":"A","to":"B","label":null}"#));
        let drag = Event::DragEnd {
            id: "B".into(),
            x: 10.04,
            y: 3.96,
        };
        assert!(line(&drag, f, 0).ends_with(r#""id":"B","x":10,"y":4}"#));
        let bad = Event::Reload {
            error: Some("line 2: oops".into()),
        };
        assert!(line(&bad, f, 0).ends_with(r#""ok":false,"error":"line 2: oops"}"#));
        let ok = Event::Reload { error: None };
        assert!(line(&ok, f, 0).ends_with(r#""ts":0,"ok":true}"#));
        assert_eq!(
            line(&Event::Quit, f, 1),
            r#"{"event":"quit","file":"/d/x.mmd","ts":1}"#
        );
        for ev in [sel, edge, drag, bad, ok, Event::Open, Event::Clear] {
            assert!(!line(&ev, f, 0).contains('\n'), "one event, one line");
        }
    }

    #[test]
    fn sink_refuses_to_append_to_the_diagram_itself() {
        let d = std::env::temp_dir().join(format!("flowcli_test_{}_self.mmd", std::process::id()));
        std::fs::write(&d, "flowchart LR\nA --> B\n").unwrap();
        let p = d.to_str().unwrap();
        assert!(Sink::open(p, p).is_err());
        // Reached through a different spelling of the same path, too.
        let dotted = d.parent().unwrap().join(".").join(d.file_name().unwrap());
        assert!(Sink::open(dotted.to_str().unwrap(), p).is_err());
        assert_eq!(
            std::fs::read_to_string(&d).unwrap(),
            "flowchart LR\nA --> B\n"
        );
        std::fs::remove_file(&d).ok();
    }

    #[test]
    fn sink_appends_whole_lines() {
        let path =
            std::env::temp_dir().join(format!("flowcli_test_{}_events.jsonl", std::process::id()));
        let p = path.to_str().unwrap();
        std::fs::write(p, "earlier\n").unwrap();
        let mut sink = Sink::open(p, "no/such/diagram.mmd").unwrap();
        assert_eq!(sink.emit(&Event::Open), None);
        assert_eq!(sink.emit(&Event::Clear), None);
        let text = std::fs::read_to_string(p).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        // Appended, never truncated; a path that can't be resolved is
        // reported as given.
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "earlier");
        assert!(lines[1].starts_with(r#"{"event":"open","file":"no/such/diagram.mmd""#));
        assert!(lines[2].starts_with(r#"{"event":"clear""#));
        std::fs::remove_file(p).ok();
    }
}
