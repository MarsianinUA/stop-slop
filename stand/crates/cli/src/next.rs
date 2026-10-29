//! The only place that knows the command strings of the stand (plan, row 83): the `next` of the
//! `--json` line, the `next:` lines of stderr and of the summary.

use std::io::{self, Write};

use report::{Line, Next};
use serde::Serialize;

use crate::{JsonLayout, guide};

pub fn render(next: &Next) -> String {
    match next {
        Next::Snapshot => "stop-slop-stand snapshot".to_owned(),
        Next::Init => "stop-slop-stand init".to_owned(),
        Next::Guide(None) => "stop-slop-stand guide".to_owned(),
        Next::Guide(Some(topic)) => format!("stop-slop-stand guide {}", guide::name(*topic)),
        // The reason is the agent's to write, so the command carries a placeholder for it. A
        // bound prints in the grammar of `--allow`.
        Next::Accept { measure, bound } => {
            format!("stop-slop-stand accept {measure} --allow {bound} --reason \"<text>\"")
        }
        Next::Reseal { before } => {
            format!("stop-slop-stand run --before {before} --reseal --reason \"<text>\"")
        }
        Next::Inspect { run, service } => {
            format!("stop-slop-stand inspect {run} head {service} -- <argv>")
        }
    }
}

/// `next: <cmd>`, one line per command: the end of stderr on 2 and 3, the end of the summary.
pub(crate) fn print_next(to: &mut dyn Write, next: &[Next]) -> io::Result<()> {
    for next in next {
        writeln!(to, "next: {}", render(next))?;
    }
    Ok(())
}

/// The line serializes without `next`; it goes here as command strings (Gate 1, «Строки `next`»).
#[derive(Serialize)]
struct Printed<'l> {
    #[serde(flatten)]
    line: &'l Line,
    next: Vec<String>,
}

pub(crate) fn print_line(to: &mut dyn Write, line: &Line, layout: JsonLayout) -> io::Result<()> {
    let printed = Printed {
        line,
        next: line.next.iter().map(render).collect(),
    };
    let text = match layout {
        JsonLayout::Line => serde_json::to_string(&printed),
        JsonLayout::Pretty => serde_json::to_string_pretty(&printed),
    };
    writeln!(to, "{}", text.expect("the line serializes"))
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use report::{Bound, Topic};

    use super::*;

    /// The `Next` whose commands exist.
    fn every_next() -> Vec<Next> {
        vec![
            Next::Snapshot,
            Next::Reseal {
                before: "session".into(),
            },
            Next::Reseal {
                before: "origin/main".into(),
            },
            Next::Inspect {
                run: "20261029-120000".into(),
                service: "db".into(),
            },
            Next::Accept {
                measure: "api:3000.connections".into(),
                bound: Bound::UpTo(15),
            },
            Next::Accept {
                measure: "db.memory".into(),
                bound: Bound::DownTo(10),
            },
            Next::Accept {
                measure: "stand.wall".into(),
                bound: Bound::None,
            },
        ]
    }

    #[test]
    fn each_next_is_a_command_the_stand_parses() {
        for next in every_next() {
            let command = render(&next);
            let args = shlex::split(&command).unwrap_or_else(|| panic!("{command} splits"));
            assert_eq!(args[0], "stop-slop-stand");
            let parsed = crate::parse(args.iter().map(OsString::from));
            assert!(parsed.is_ok(), "{command}: {:?}", parsed.err());
        }
    }

    #[test]
    fn an_accept_bound_reads_back_as_the_change_it_names() {
        let bounds = [
            (Bound::UpTo(15), "+15%"),
            (Bound::DownTo(10), "-10%"),
            (Bound::None, "none"),
            (Bound::Down, "down"),
        ];
        for (bound, text) in bounds {
            let next = Next::Accept {
                measure: "api:3000.connections".into(),
                bound,
            };
            let args = shlex::split(&render(&next)).unwrap();
            let allow = args.iter().position(|arg| arg == "--allow").unwrap();
            assert_eq!(args[allow + 1], text);
            assert!(spec::Change::parse(text).is_ok(), "{text}");
        }
        assert_eq!(
            render(&Next::Guide(Some(Topic::Recipe))),
            "stop-slop-stand guide recipe"
        );
    }
}
