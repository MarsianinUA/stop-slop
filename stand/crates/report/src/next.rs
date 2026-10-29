//! The next steps of a line as types; only `cli` turns them into command strings (plan, row 83).

use crate::context::{Bound, Intent};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Snapshot,
    Init,
    Guide(Option<Topic>),
    /// The template of `accept` for a `surprise` or an `exceeded`; `bound` is the shift rounded up
    /// to a multiple of 5%, never wider than `maxTolerance`.
    Accept {
        measure: String,
        bound: Bound,
    },
    /// `run --before <before> --reseal --reason ..`: the command needs the intent it reseals.
    Reseal {
        before: Intent,
    },
    /// Only with a head failure (exit 1): a service whose failure snapshot `kept.json` holds.
    Inspect {
        run: String,
        service: String,
    },
}

/// The topics of `guide`: the sections of `guide.md`, and `Probes` for `probes.md`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Topic {
    Recipe,
    Probes,
    Expect,
    Line,
    Inspect,
}
