//! The folder `preload.cjs` writes. Per Node process `<id>` (its pid, `<pid>-t<threadId>` for a
//! worker thread, and `-r<n>` after it when an earlier process of the run had that pid):
//! `<id>.start.json` once the preload runs; then for chunk `n` from 0, `<id>.<n>.maps.json` with
//! the maps of the scripts first seen in it, when there are any, and `<id>.<n>.cpuprofile`,
//! V8's `Profiler.Profile`. The chunk files are renamed into place whole.

use std::borrow::Cow;
use std::collections::{BTreeMap, HashMap};
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;

use crate::maps::{self, Entry, Map};
use crate::{HotLine, ModuleTime, ProfileError, Sampled, Samples};

const TS_EXTENSIONS: [&str; 4] = ["ts", "tsx", "mts", "cts"];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Profile<'a> {
    #[serde(borrow)]
    nodes: Vec<Node<'a>>,
    /// Microseconds, as the deltas.
    start_time: f64,
    end_time: f64,
    samples: Vec<u32>,
    time_deltas: Vec<f64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Node<'a> {
    id: u32,
    #[serde(borrow)]
    call_frame: CallFrame<'a>,
    #[serde(default)]
    children: Vec<u32>,
    /// Samples of this node as the top frame, by line of the generated code from 1.
    #[serde(default)]
    position_ticks: Vec<PositionTick>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CallFrame<'a> {
    #[serde(borrow)]
    url: Cow<'a, str>,
    /// From 0; -1 for V8's own frames such as `(program)`.
    line_number: i64,
    column_number: i64,
}

#[derive(Deserialize)]
struct PositionTick {
    line: u32,
    ticks: u32,
}

#[derive(Default)]
struct Process {
    chunks: BTreeMap<u32, Utf8PathBuf>,
    maps: BTreeMap<u32, Utf8PathBuf>,
}

pub(crate) fn read(dir: &Utf8Path, root: &Utf8Path) -> Result<Samples, ProfileError> {
    let processes = list(dir)?;
    if processes.values().all(|process| process.chunks.is_empty()) {
        return Err(ProfileError::NoProfile);
    }
    let mut sums = Sums::new(root);
    for process in processes.values() {
        let mut scripts = Scripts::load(&process.maps)?;
        for path in process.chunks.values() {
            let bytes = fs::read(path).map_err(ProfileError::io(path))?;
            let profile: Profile<'_> =
                serde_json::from_slice(&bytes).map_err(|error| bad(path, error.to_string()))?;
            sums.add(&profile, &mut scripts)
                .map_err(|message| bad(path, message))?;
        }
    }
    let lost = processes
        .values()
        .filter(|process| process.chunks.is_empty())
        .count();
    Ok(sums.finish(count(processes.len()), count(lost)))
}

/// By process. Other names, such as `preload.cjs` or a `.tmp` that a kill left, are skipped.
fn list(dir: &Utf8Path) -> Result<BTreeMap<String, Process>, ProfileError> {
    let mut processes: BTreeMap<String, Process> = BTreeMap::new();
    for entry in dir.read_dir_utf8().map_err(ProfileError::io(dir))? {
        let entry = entry.map_err(ProfileError::io(dir))?;
        let Some((id, rest)) = entry.file_name().split_once('.') else {
            continue;
        };
        if rest == "start.json" {
            processes.entry(id.to_owned()).or_default();
            continue;
        }
        let Some((n, kind)) = rest.split_once('.') else {
            continue;
        };
        let Ok(n) = n.parse::<u32>() else {
            continue;
        };
        let is_chunk = match kind {
            "cpuprofile" => true,
            "maps.json" => false,
            _ => continue,
        };
        let process = processes.entry(id.to_owned()).or_default();
        let files = if is_chunk {
            &mut process.chunks
        } else {
            &mut process.maps
        };
        files.insert(n, entry.into_path());
    }
    Ok(processes)
}

/// The scripts of one process by URL. A map never serves another process: two jest projects
/// may compile one file differently.
struct Scripts {
    entries: HashMap<String, Entry>,
    known: HashMap<String, Script>,
}

struct Script {
    /// `None` for `node:` modules and V8's own frames.
    file: Option<Utf8PathBuf>,
    map: Option<Map>,
}

impl Scripts {
    fn load(maps: &BTreeMap<u32, Utf8PathBuf>) -> Result<Scripts, ProfileError> {
        let mut entries = HashMap::new();
        for path in maps.values() {
            let bytes = fs::read(path).map_err(ProfileError::io(path))?;
            let chunk: HashMap<String, Entry> =
                serde_json::from_slice(&bytes).map_err(|error| bad(path, error.to_string()))?;
            for (url, entry) in chunk {
                entries.entry(url).or_insert(entry);
            }
        }
        Ok(Scripts {
            entries,
            known: HashMap::new(),
        })
    }

    fn get(&mut self, url: &str) -> &Script {
        if !self.known.contains_key(url) {
            let file = maps::file_path(url);
            let map = file.as_deref().and_then(|file| {
                let entry = self.entries.remove(url)?;
                Map::load(entry, file)
            });
            self.known.insert(url.to_owned(), Script { file, map });
        }
        &self.known[url]
    }
}

/// What a frame counts as. Files are indexes into `Sums::files`, lines and columns from 1.
#[derive(Clone, Copy)]
enum Frame {
    Function {
        file: u32,
        line: u32,
        column: u32,
    },
    Module {
        file: u32,
    },
    /// Local `.ts` code that found no map.
    Unmapped,
    /// Outside `root`, in `node_modules`, or not a file.
    Other,
}

#[derive(Default)]
struct Function {
    self_us: f64,
    total_us: f64,
    lines: HashMap<u32, f64>,
}

/// Microseconds until `finish`.
struct Sums {
    root: Utf8PathBuf,
    total: f64,
    local: f64,
    unmapped: f64,
    files: Vec<Utf8PathBuf>,
    file_ids: HashMap<Utf8PathBuf, u32>,
    functions: HashMap<(u32, u32, u32), Function>,
    modules: HashMap<u32, f64>,
}

impl Sums {
    fn new(root: &Utf8Path) -> Sums {
        Sums {
            root: maps::normalize(root),
            total: 0.0,
            local: 0.0,
            unmapped: 0.0,
            files: Vec::new(),
            file_ids: HashMap::new(),
            functions: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    fn add(&mut self, profile: &Profile<'_>, scripts: &mut Scripts) -> Result<(), String> {
        let nodes = &profile.nodes;
        let index: HashMap<u32, usize> = nodes
            .iter()
            .enumerate()
            .map(|(at, node)| (node.id, at))
            .collect();
        let mut parents = vec![None; nodes.len()];
        for (at, node) in nodes.iter().enumerate() {
            for child in &node.children {
                if let Some(&child) = index.get(child) {
                    parents[child] = Some(at);
                }
            }
        }

        // A sample lasts until the next one; the last one until the end of the profile.
        let mut self_us = vec![0.0; nodes.len()];
        let mut stamp = profile.start_time;
        for (k, id) in profile.samples.iter().enumerate() {
            stamp += profile.time_deltas.get(k).copied().unwrap_or(0.0);
            let next = profile
                .time_deltas
                .get(k + 1)
                .map_or(profile.end_time, |delta| stamp + delta);
            let at = index
                .get(id)
                .ok_or_else(|| format!("sample of node {id}, which `nodes` does not have"))?;
            self_us[*at] += (next - stamp).max(0.0);
        }

        let frames: Vec<(Frame, Vec<(u32, u32)>)> = nodes
            .iter()
            .map(|node| self.resolve(node, scripts))
            .collect();
        let mut counted = Vec::new();
        for (leaf, &us) in self_us.iter().enumerate().filter(|(_, us)| **us > 0.0) {
            self.total += us;
            let (frame, lines) = &frames[leaf];
            match *frame {
                Frame::Function { file, line, column } => {
                    self.local += us;
                    let function = self.functions.entry((file, line, column)).or_default();
                    function.self_us += us;
                    let ticks: u32 = nodes[leaf]
                        .position_ticks
                        .iter()
                        .map(|tick| tick.ticks)
                        .sum();
                    for &(line, line_ticks) in lines {
                        *function.lines.entry(line).or_default() +=
                            us * f64::from(line_ticks) / f64::from(ticks);
                    }
                }
                Frame::Module { file } => {
                    self.local += us;
                    *self.modules.entry(file).or_default() += us;
                }
                Frame::Unmapped => {
                    self.local += us;
                    self.unmapped += us;
                }
                Frame::Other => {}
            }
            counted.clear();
            let mut at = Some(leaf);
            while let Some(node) = at {
                if let Frame::Function { file, line, column } = frames[node].0 {
                    let key = (file, line, column);
                    if !counted.contains(&key) {
                        counted.push(key);
                        self.functions.entry(key).or_default().total_us += us;
                    }
                }
                at = parents[node];
            }
        }
        Ok(())
    }

    /// The frame of `node` and, for a local function, its sampled lines in the original file
    /// with their ticks.
    fn resolve(&mut self, node: &Node<'_>, scripts: &mut Scripts) -> (Frame, Vec<(u32, u32)>) {
        let call = &node.call_frame;
        let script = scripts.get(&call.url);
        let (Some(file), Ok(line), Ok(column)) = (
            script.file.as_deref(),
            u32::try_from(call.line_number),
            u32::try_from(call.column_number),
        ) else {
            return (Frame::Other, Vec::new());
        };
        // V8 starts the top-level code of a script at 0:0.
        let top_level = line == 0 && column == 0;
        if let Some(map) = &script.map
            && let Some(module) = map.module()
        {
            let position = if top_level {
                None
            } else {
                map.find(line, column)
            };
            // Code the map has no original for is what the compiler added around the file:
            // tsc's `__decorate` and `__exportStar` run while the module loads.
            let Some((source, line, column)) = position else {
                return (self.module(module), Vec::new());
            };
            let frame = self.function(source, line + 1, column + 1);
            let lines = lines(node, frame, |line| {
                let (in_file, line) = map.line(line.checked_sub(1)?)?;
                (in_file == source).then_some(line + 1)
            });
            return (frame, lines);
        }
        if file
            .extension()
            .is_some_and(|extension| TS_EXTENSIONS.contains(&extension))
        {
            let frame = if self.is_local(file) {
                Frame::Unmapped
            } else {
                Frame::Other
            };
            return (frame, Vec::new());
        }
        if top_level {
            return (self.module(file), Vec::new());
        }
        let frame = self.function(file, line + 1, column + 1);
        (frame, lines(node, frame, Some))
    }

    fn function(&mut self, file: &Utf8Path, line: u32, column: u32) -> Frame {
        match self.local_id(file) {
            Some(file) => Frame::Function { file, line, column },
            None => Frame::Other,
        }
    }

    fn module(&mut self, file: &Utf8Path) -> Frame {
        match self.local_id(file) {
            Some(file) => Frame::Module { file },
            None => Frame::Other,
        }
    }

    fn local_id(&mut self, file: &Utf8Path) -> Option<u32> {
        if let Some(&id) = self.file_ids.get(file) {
            return Some(id);
        }
        if !self.is_local(file) {
            return None;
        }
        let id = count(self.files.len());
        self.files.push(file.to_owned());
        self.file_ids.insert(file.to_owned(), id);
        Some(id)
    }

    /// Under `root` and outside `node_modules`; Windows ignores the case of file names.
    fn is_local(&self, file: &Utf8Path) -> bool {
        let mut parts = file.components();
        for root_part in self.root.components() {
            match parts.next() {
                Some(part) if same_name(part.as_str(), root_part.as_str()) => {}
                _ => return false,
            }
        }
        parts.all(|part| part.as_str() != "node_modules")
    }

    fn finish(self, processes: u32, lost_processes: u32) -> Samples {
        let ms = |us: f64| us / 1000.0;
        let mut functions: Vec<Sampled> = self
            .functions
            .into_iter()
            .map(|((file, line, column), function)| {
                let mut lines: Vec<HotLine> = function
                    .lines
                    .into_iter()
                    .map(|(line, us)| HotLine { line, ms: ms(us) })
                    .collect();
                lines.sort_by(|a, b| b.ms.total_cmp(&a.ms).then(a.line.cmp(&b.line)));
                Sampled {
                    file: self.files[file as usize].clone(),
                    line,
                    column,
                    self_ms: ms(function.self_us),
                    total_ms: ms(function.total_us),
                    lines,
                }
            })
            .collect();
        functions.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));
        let mut modules: Vec<ModuleTime> = self
            .modules
            .into_iter()
            .map(|(file, us)| ModuleTime {
                file: self.files[file as usize].clone(),
                self_ms: ms(us),
            })
            .collect();
        modules.sort_by(|a, b| a.file.cmp(&b.file));
        Samples {
            total_ms: ms(self.total),
            local_ms: ms(self.local),
            unmapped_ms: ms(self.unmapped),
            processes,
            lost_processes,
            functions,
            modules,
        }
    }
}

/// The sampled lines of a local function, from 1, with their ticks; `original` turns a line
/// of the generated code into one of the source.
fn lines(node: &Node<'_>, frame: Frame, original: impl Fn(u32) -> Option<u32>) -> Vec<(u32, u32)> {
    if !matches!(frame, Frame::Function { .. }) {
        return Vec::new();
    }
    node.position_ticks
        .iter()
        .filter_map(|tick| Some((original(tick.line)?, tick.ticks)))
        .collect()
}

fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn bad(path: &Utf8Path, message: String) -> ProfileError {
    ProfileError::BadProfile {
        path: path.to_owned(),
        message,
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
