//! The topics of `guide`.

use report::Topic;

pub(crate) fn name(topic: Topic) -> &'static str {
    match topic {
        Topic::Recipe => "recipe",
        Topic::Probes => "probes",
        Topic::Expect => "expect",
        Topic::Line => "line",
        Topic::Inspect => "inspect",
    }
}
