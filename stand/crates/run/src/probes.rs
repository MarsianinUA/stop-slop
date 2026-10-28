//! The probes of a stand (architecture plan, row 62): the output of each is
//! `<out>/probe.<name>.jsonl`.

use spec::ProbeName;

pub(crate) fn file_name(name: &ProbeName) -> String {
    format!("probe.{}.jsonl", name.as_str())
}
