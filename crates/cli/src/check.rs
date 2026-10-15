use std::collections::BTreeMap;
use std::env;
use std::panic::resume_unwind;
use std::thread;

use camino::{Utf8Path, Utf8PathBuf};
use engine::{Engine, Finding, FixCount, LineRange, Mode, RunConfig, SourceFile};
use kit::{EnumIndex, Fact, Facts, Package};
use project::ProjectFacts;
use repo::{Repo, RepoError, Snapshot, WorkTree};
use scope::{Base, GhPrBase, Scope, ScopeRequest, ScopedFile, Skipped};
use state::State;

use crate::args::CheckArgs;
use crate::error::{CliError, Exit};
use crate::output;

/// `files` holds only the files that were read.
pub(crate) struct Checked {
    pub(crate) base: Option<Base>,
    pub(crate) files: Vec<Utf8PathBuf>,
    pub(crate) skipped: Vec<Skipped>,
    pub(crate) findings: Vec<Finding>,
    pub(crate) fixed: Vec<FixCount>,
}

pub(crate) fn run(args: CheckArgs) -> Result<Exit, CliError> {
    let json = args.json;
    let checked = check(args)?;
    let text = if json {
        output::check_json(&checked)
    } else {
        output::check_text(&checked)
    };
    output::print(&text);
    Ok(if checked.findings.is_empty() {
        Exit::Clean
    } else {
        Exit::Found
    })
}

fn check(args: CheckArgs) -> Result<Checked, CliError> {
    let cwd = env::current_dir().map_err(|source| RepoError::Io {
        path: ".".into(),
        source,
    })?;
    let repo = Repo::open(&cwd)?;
    let state = State::open(repo.git_common_dir());
    let config = state.config()?;
    let ignored = state
        .ignores()?
        .into_iter()
        .map(|entry| (entry.file, entry.fingerprint))
        .collect();
    // Before `scope`: a broken `config.toml` fails before the git calls of the diff.
    let engine = Engine::new(
        &rules::RULESET,
        RunConfig {
            disabled: config.disabled,
            options: config.rules,
            ignored,
        },
    )?;
    let snapshot = match &args.rev {
        Some(rev) => repo.at_rev(rev)?,
        None => repo.worktree(),
    };
    let fix = args.fix;
    // Built once for `Fix` and `Check`: `Fix` does not edit `package.json`, `tsconfig.json`
    // or the values of enums, and `Check` runs every rule `Fix` runs.
    let mut needed = engine.facts_needed(Mode::Check);
    let enums_needed = needed.contains(&Fact::Enums);
    needed.retain(|fact| *fact != Fact::Enums);

    // The enum index greps the whole repo and does not depend on the files, so it runs
    // beside `scope` and is joined last: on a diff of 30 files 95 ms instead of 150 in turn.
    let (prepared, index) = thread::scope(|threads| {
        let index = enums_needed
            .then(|| threads.spawn(|| ProjectFacts::load(&snapshot, &[], &[Fact::Enums])));
        let prepared = prepare(&repo, &snapshot, request(args), &needed);
        let index = index.map(|index| index.join().unwrap_or_else(|panic| resume_unwind(panic)));
        (prepared, index)
    });
    // A `scope` error names the base or the path the user gave, so it goes first.
    let (mut scope, facts, mut sources) = prepared?;
    let index = index.transpose()?;
    let joined = Joined {
        files: &facts,
        // Without `Fact::Enums` no thread ran, and the empty index of `facts` stands in.
        enums: index.as_ref().unwrap_or(&facts),
    };

    let mut fixed = Vec::new();
    if let Snapshot::WorkTree(tree) = &snapshot
        && fix
    {
        fixed = write_fixes(&engine, tree, &mut sources, &joined)?;
        if !fixed.is_empty()
            && let Some(base) = &scope.base
        {
            scope.skipped = rescope(&repo, &snapshot, base, &mut sources)?;
        }
    }
    let files = sources.iter().map(|source| source.path.clone()).collect();
    let report = engine.run(sources, &joined, Mode::Check);
    Ok(Checked {
        base: scope.base,
        files,
        skipped: scope.skipped,
        findings: report.findings,
        fixed,
    })
}

/// Writes only the files `Fix` changed. Their `sources` get the written bytes, so `Check`
/// reads nothing again.
fn write_fixes(
    engine: &Engine,
    tree: &WorkTree,
    sources: &mut [SourceFile],
    facts: &dyn Facts,
) -> Result<Vec<FixCount>, CliError> {
    let copies = sources
        .iter()
        .map(|source| SourceFile {
            path: source.path.clone(),
            bytes: source.bytes.clone(),
            changed: None,
        })
        .collect();
    let report = engine.run(copies, facts, Mode::Fix);
    let mut rewritten: BTreeMap<_, _> = report
        .rewritten
        .into_iter()
        .map(|file| (file.path, file.bytes))
        .collect();
    for source in sources {
        if let Some(bytes) = rewritten.remove(&source.path) {
            tree.write(&source.path, &bytes)?;
            source.bytes = bytes;
        }
    }
    Ok(report.fixed)
}

/// Takes the changed lines of `sources` and the skipped files anew after `Fix` wrote them.
fn rescope(
    repo: &Repo,
    snapshot: &Snapshot,
    base: &Base,
    sources: &mut Vec<SourceFile>,
) -> Result<Vec<Skipped>, CliError> {
    // The oid of the first base, not a new search: no second `gh` call (half a second of
    // network) and no second ancestor search.
    let request = ScopeRequest::Branch {
        base: Some(base.oid.to_string()),
    };
    let again = scope::resolve(repo, snapshot, &GhPrBase, request)?;
    let mut changed: BTreeMap<_, _> = again
        .files
        .into_iter()
        .map(|file| (file.path, file.changed))
        .collect();
    // A file that became as in the base drops out; `Fix` creates no files, so none comes in.
    sources.retain_mut(|source| match changed.remove(&source.path) {
        Some(lines) => {
            source.changed = lines.map(engine_lines);
            true
        }
        None => false,
    });
    Ok(again.skipped)
}

fn request(args: CheckArgs) -> ScopeRequest {
    if !args.files.is_empty() {
        ScopeRequest::Files(args.files)
    } else if args.all {
        ScopeRequest::All
    } else {
        ScopeRequest::Branch { base: args.base }
    }
}

/// The scope with its files taken out, the facts of the files and the files themselves.
fn prepare(
    repo: &Repo,
    snapshot: &Snapshot,
    request: ScopeRequest,
    needed: &[Fact],
) -> Result<(Scope, ProjectFacts, Vec<SourceFile>), CliError> {
    let mut scope = scope::resolve(repo, snapshot, &GhPrBase, request)?;
    let files = std::mem::take(&mut scope.files);
    let paths: Vec<Utf8PathBuf> = files.iter().map(|file| file.path.clone()).collect();
    let facts = ProjectFacts::load(snapshot, &paths, needed)?;
    let bytes = snapshot.read(&paths)?;
    Ok((scope, facts, sources(files, bytes)))
}

/// `None` is a file of the index removed from the disk (`--all`): it is not checked.
fn sources(files: Vec<ScopedFile>, bytes: Vec<Option<Vec<u8>>>) -> Vec<SourceFile> {
    files
        .into_iter()
        .zip(bytes)
        .filter_map(|(file, bytes)| {
            Some(SourceFile {
                path: file.path,
                bytes: bytes?,
                changed: file.changed.map(engine_lines),
            })
        })
        .collect()
}

fn engine_lines(lines: Vec<repo::LineRange>) -> Vec<LineRange> {
    lines
        .into_iter()
        .map(|line| LineRange {
            start: line.start,
            end: line.end,
        })
        .collect()
}

struct Joined<'f> {
    files: &'f ProjectFacts,
    enums: &'f ProjectFacts,
}

impl Facts for Joined<'_> {
    fn package(&self, file: &Utf8Path) -> Option<&Package> {
        self.files.package(file)
    }

    fn aliases(&self, file: &Utf8Path) -> &[String] {
        self.files.aliases(file)
    }

    fn enums(&self) -> &EnumIndex {
        self.enums.enums()
    }
}
