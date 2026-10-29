//! The next steps of a line as types; only `cli` turns them into command strings (plan, row 83).

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    Snapshot,
    Init,
    Guide(Option<Topic>),
    /// `run --before <before> --reseal --reason ..`: the command needs the intent it reseals.
    Reseal {
        before: String,
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
