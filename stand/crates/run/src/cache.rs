//! The setup cache entry of a version (plan, row 88). The folder holds `entry.json` and `out/`,
//! nothing else: a restore of its snapshots takes the stand's paths and the harness from the folders
//! of the run that restores them, and the key's recipe digest pins what the harness holds.

use std::collections::BTreeMap;
use std::fs;

use camino::Utf8Path;
use report::{ProbeRan, SetupSide};
use sandbox::{ExecOutcome, SnapshotId};
use serde::{Deserialize, Serialize};

use crate::StandFault;
use crate::launch::Seeds;

const ENTRY: &str = "entry.json";
const OUT: &str = "out";

/// The shape is the writer's own (blueprint, Law vs Recommendation): only `run` reads it.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryFile {
    version: Option<SnapshotId>,
    /// The setup snapshot of each service with a volume, by service.
    volumes: BTreeMap<String, SnapshotId>,
    /// By volume.
    sizes: BTreeMap<String, u64>,
    /// The `after-setup` probes.
    probes: Vec<EntryProbe>,
}

#[derive(Serialize, Deserialize)]
struct EntryProbe {
    name: String,
    outcome: ExecOutcome,
    /// In `out/`.
    file: String,
}

/// The snapshots `entry/entry.json` lists; `None` without that file: the setup of the entry did
/// not pass.
pub fn cached_ids(entry: &Utf8Path) -> Result<Option<Vec<SnapshotId>>, StandFault> {
    let file = read(entry)?;
    Ok(file.map(|file| {
        file.version
            .into_iter()
            .chain(file.volumes.into_values())
            .collect()
    }))
}

/// Once the whole setup of a version passed: its outputs first, then `entry.json`, so its presence
/// says the entry is whole.
pub(crate) fn write(entry: &Utf8Path, seeds: &Seeds, setup: &SetupSide) -> Result<(), StandFault> {
    copy_dir(&setup.out_dir, &entry.join(OUT))?;
    let probes = setup.probes.iter().map(|probe| EntryProbe {
        name: probe.name.clone(),
        outcome: probe.outcome,
        file: probe
            .out
            .file_name()
            .expect("a probe writes a file")
            .to_owned(),
    });
    let file = EntryFile {
        version: seeds.version.clone(),
        volumes: seeds.volumes.clone(),
        sizes: setup.sizes.clone(),
        probes: probes.collect(),
    };
    crate::write_whole(&entry.join(ENTRY), &file)
}

/// A hit: what the repeats start from, and the outputs of the setup that built the entry, its
/// `out/` copied into `out_dir`, so the run folder stays whole after the entry goes.
pub(crate) fn hit(entry: &Utf8Path, out_dir: &Utf8Path) -> Result<(Seeds, SetupSide), StandFault> {
    let path = entry.join(ENTRY);
    let file = read(entry)?.ok_or_else(|| {
        StandFault::bad(
            &path,
            "not found, and only the entry of a passed setup is a hit",
        )
    })?;
    copy_dir(&entry.join(OUT), out_dir)?;
    let probes = file.probes.into_iter().map(|probe| ProbeRan {
        name: probe.name,
        outcome: probe.outcome,
        out: out_dir.join(probe.file),
    });
    let seeds = Seeds {
        version: file.version,
        volumes: file.volumes,
    };
    let setup = SetupSide {
        sizes: file.sizes,
        probes: probes.collect(),
        out_dir: out_dir.to_owned(),
    };
    Ok((seeds, setup))
}

fn read(entry: &Utf8Path) -> Result<Option<EntryFile>, StandFault> {
    crate::read_json(&entry.join(ENTRY))
}

/// The whole of `/stand/out` of setup: `.err` and `.status` lie beside each probe output, and a
/// service with a probe may write there too. Only folders and plain files are copied; anything
/// else is skipped and never followed, as a link would read the host, unless it is a top-level
/// `*.jsonl`, which the report refuses: refused here too, on a build and on a hit alike.
fn copy_dir(from: &Utf8Path, to: &Utf8Path) -> Result<(), StandFault> {
    copy_tree(from, to, true)
}

fn copy_tree(from: &Utf8Path, to: &Utf8Path, top: bool) -> Result<(), StandFault> {
    fs::create_dir_all(to).map_err(StandFault::io(to))?;
    for entry in from.read_dir_utf8().map_err(StandFault::io(from))? {
        let entry = entry.map_err(StandFault::io(from))?;
        let target = to.join(entry.file_name());
        let kind = entry.file_type().map_err(StandFault::io(entry.path()))?;
        if kind.is_dir() {
            copy_tree(entry.path(), &target, false)?;
        } else if kind.is_file() {
            fs::copy(entry.path(), &target).map_err(StandFault::io(&target))?;
        } else if top && entry.file_name().ends_with(".jsonl") {
            return Err(StandFault::bad(
                entry.path(),
                "not a plain file: the stand reads the results in /stand/out only as plain \
                 files, never through a link",
            ));
        }
    }
    Ok(())
}
