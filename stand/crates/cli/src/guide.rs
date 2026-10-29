//! `guide [topic]` and `init`. The text of `guide` is the skill's own reference files, built into
//! the binary: one copy, which the agent reads either way (plan, row 75).

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};

use camino::Utf8Path;
use report::Topic;

use crate::error::StandError;
use crate::{Env, Exit, slashed, stand_file};

const GUIDE: &str = include_str!("../../../../skill/references/guide.md");
const PROBES: &str = include_str!("../../../../skill/references/probes.md");
const TEMPLATE: &str = include_str!("template.yaml");

/// In the order of the contents. `probes` is the whole of `probes.md`; the others are the `## `
/// sections of `guide.md`.
pub(crate) const TOPICS: [(&str, Topic); 5] = [
    ("recipe", Topic::Recipe),
    ("probes", Topic::Probes),
    ("expect", Topic::Expect),
    ("line", Topic::Line),
    ("inspect", Topic::Inspect),
];

pub(crate) fn name(topic: Topic) -> &'static str {
    match topic {
        Topic::Recipe => "recipe",
        Topic::Probes => "probes",
        Topic::Expect => "expect",
        Topic::Line => "line",
        Topic::Inspect => "inspect",
    }
}

pub(crate) fn names() -> String {
    TOPICS.map(|(name, _)| name).join(", ")
}

pub(crate) fn run(topic: Option<Topic>, env: &mut Env<'_>) -> Result<Exit, StandError> {
    let text = match topic {
        None => format!(
            "{}\n\nTopics: {}. `stop-slop-stand guide <topic>` prints one.\n",
            parts(GUIDE).0.trim_end(),
            names()
        ),
        Some(Topic::Probes) => PROBES.to_owned(),
        Some(topic) => section(name(topic))
            .expect("every topic but probes is a section of guide.md")
            .to_owned(),
    };
    write!(env.out, "{text}").map_err(|source| StandError::Io {
        path: "stdout".into(),
        source,
    })?;
    Ok(Exit::Passed)
}

/// `guide.md` cut at its `## ` lines: the shortest path before the first, then each section with
/// its heading. A line may end in `\r\n`: git checks text out so on Windows.
fn parts(text: &str) -> (&str, Vec<(&str, &str)>) {
    let mut starts = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if let Some(name) = line.trim_end().strip_prefix("## ") {
            starts.push((name, at));
        }
        at += line.len();
    }
    let intro = &text[..starts.first().map_or(text.len(), |(_, at)| *at)];
    let ends = starts.iter().skip(1).map(|(_, at)| *at).chain([text.len()]);
    let sections = starts
        .iter()
        .zip(ends)
        .map(|((name, start), end)| (*name, &text[*start..end]))
        .collect();
    (intro, sections)
}

fn section(name: &str) -> Option<&'static str> {
    let (_, sections) = parts(GUIDE);
    sections
        .into_iter()
        .find_map(|(heading, text)| (heading == name).then_some(text))
}

/// Writes the template, never over a stand file that is there (plan, row 75).
pub(crate) fn init(env: &mut Env<'_>) -> Result<Exit, StandError> {
    let worktree = source::open(env.cwd)?;
    let file = stand_file(env.cwd, &worktree);
    let folder = file.parent().expect("the stand file lies in its folder");
    let io = |path: &Utf8Path| {
        let path = path.to_owned();
        move |source| StandError::Io { path, source }
    };
    fs::create_dir_all(folder).map_err(io(folder))?;
    let created = OpenOptions::new().write(true).create_new(true).open(&file);
    let mut handle = match created {
        Ok(handle) => handle,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(StandError::StandFileExists(file));
        }
        Err(error) => return Err(io(&file)(error)),
    };
    handle.write_all(TEMPLATE.as_bytes()).map_err(io(&file))?;
    writeln!(env.out, "wrote {}", slashed(&file)).map_err(io(Utf8Path::new("stdout")))?;
    Ok(Exit::Passed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_section_of_guide_md_is_a_topic_and_every_topic_prints() {
        let (intro, sections) = parts(GUIDE);
        let mut headings: Vec<&str> = sections.iter().map(|(name, _)| *name).collect();
        headings.sort_unstable();
        let mut expected: Vec<&str> = TOPICS
            .iter()
            .map(|(name, _)| *name)
            .filter(|name| *name != "probes")
            .collect();
        expected.sort_unstable();
        assert_eq!(headings, expected, "the `## ` sections of guide.md");
        for (name, topic) in TOPICS {
            assert_eq!(super::name(topic), name);
            if topic != Topic::Probes {
                let text = section(name).unwrap();
                let mut lines = text.lines();
                assert_eq!(lines.next(), Some(format!("## {name}").as_str()));
                assert!(lines.all(|line| !line.starts_with("## ")), "{name}");
            }
        }
        assert!(!intro.trim().is_empty());
        assert!(intro.lines().all(|line| !line.starts_with("## ")));
        assert!(PROBES.contains("$$"));
    }

    #[test]
    fn a_guide_checked_out_with_crlf_cuts_the_same() {
        let lf = GUIDE.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        let (intro, sections) = parts(&crlf);
        let (lf_intro, lf_sections) = parts(&lf);
        assert_eq!(intro.replace("\r\n", "\n"), lf_intro);
        let unix = |sections: Vec<(&str, &str)>| -> Vec<(String, String)> {
            let pairs = sections.into_iter();
            pairs
                .map(|(name, text)| (name.to_owned(), text.replace("\r\n", "\n")))
                .collect()
        };
        assert_eq!(unix(sections), unix(lf_sections));
    }

    #[test]
    fn the_template_loads() {
        let dir = tempfile::tempdir().unwrap();
        let file = Utf8Path::from_path(dir.path())
            .unwrap()
            .join("compose.yaml");
        let lf = TEMPLATE.replace("\r\n", "\n");
        for text in [lf.clone(), lf.replace('\n', "\r\n")] {
            fs::write(&file, text).unwrap();
            let loaded = spec::load(&file, &spec::Policy::new(None).unwrap());
            assert!(loaded.is_ok(), "{:?}", loaded.err());
        }
    }
}
