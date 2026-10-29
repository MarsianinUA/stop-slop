//! `guide [topic]` and `init`. The text of `guide` is the skill's own reference files, built into
//! the binary: one copy, which the agent reads either way (plan, row 75).

use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Write};

use camino::Utf8Path;
use report::Topic;

use crate::error::StandError;
use crate::{Exit, slashed, stand_file};

const GUIDE: &str = include_str!("../../../../skill/references/guide.md");
const PROBES: &str = include_str!("../../../../skill/references/probes.md");
const TEMPLATE: &str = include_str!("template.yaml");

/// In the order of the contents. `probes` is the whole of `probes.md`; the others are the `## `
/// sections of `guide.md`.
pub(crate) const TOPICS: [Topic; 5] = [
    Topic::Recipe,
    Topic::Probes,
    Topic::Expect,
    Topic::Line,
    Topic::Inspect,
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
    TOPICS.map(name).join(", ")
}

pub(crate) fn run(topic: Option<Topic>, out: &mut dyn Write) -> Result<Exit, StandError> {
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
    write!(out, "{text}").map_err(StandError::io("stdout"))?;
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
pub(crate) fn init(cwd: &Utf8Path, out: &mut dyn Write) -> Result<Exit, StandError> {
    let worktree = source::open(cwd)?;
    let file = stand_file(cwd, &worktree);
    let folder = file.parent().expect("the stand file lies in its folder");
    fs::create_dir_all(folder).map_err(StandError::io(folder))?;
    let created = OpenOptions::new().write(true).create_new(true).open(&file);
    let mut handle = match created {
        Ok(handle) => handle,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            return Err(StandError::StandFileExists(file));
        }
        Err(error) => return Err(StandError::io(&file)(error)),
    };
    if let Err(error) = handle.write_all(TEMPLATE.as_bytes()) {
        // A part of the template would refuse the next `init` and fail every `run`.
        drop(handle);
        let _ = fs::remove_file(&file);
        return Err(StandError::io(&file)(error));
    }
    writeln!(out, "wrote {}", slashed(&file)).map_err(StandError::io("stdout"))?;
    Ok(Exit::Passed)
}

#[cfg(test)]
mod tests {
    use camino::Utf8Path;

    use super::*;

    #[test]
    fn every_section_of_guide_md_is_a_topic_and_every_topic_prints() {
        let (intro, sections) = parts(GUIDE);
        let mut headings: Vec<&str> = sections.iter().map(|(name, _)| *name).collect();
        headings.sort_unstable();
        let mut expected: Vec<&str> = TOPICS
            .into_iter()
            .filter(|topic| *topic != Topic::Probes)
            .map(name)
            .collect();
        expected.sort_unstable();
        assert_eq!(headings, expected, "the `## ` sections of guide.md");
        for topic in TOPICS {
            let name = name(topic);
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

    /// 8 logical CPUs (a MacBook) give a stand 4 vCPUs, what the setup of the template asks; its
    /// services ask 6 in all.
    #[test]
    fn the_template_fits_a_host_of_eight_cpus() {
        let dir = tempfile::tempdir().unwrap();
        let file = Utf8Path::from_path(dir.path())
            .unwrap()
            .join("compose.yaml");
        fs::write(&file, TEMPLATE).unwrap();
        let stand = spec::load(&file, &spec::Policy::new(None).unwrap()).unwrap();
        const GIB: u64 = 1 << 30;
        let host = budget::Host {
            cpus: 8,
            memory: 16 * GIB,
            free_memory: 12 * GIB,
            free_disk: 100 * GIB,
        };
        let admitted = budget::check(
            crate::measure::demand(&stand),
            &host,
            budget::Share::new(None).unwrap(),
        )
        .unwrap();
        assert_eq!((admitted.cpus, admitted.memory), (4, 3 * GIB));
    }
}
