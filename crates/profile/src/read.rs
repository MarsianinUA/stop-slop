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

use crate::maps::{self, Scripts};
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

/// Sums the profiles `run` left in `dir` by function of the original sources.
pub fn read(dir: &Utf8Path, root: &Utf8Path) -> Result<Samples, ProfileError> {
    let processes = list(dir)?;
    let lost = processes
        .values()
        .filter(|process| process.chunks.is_empty())
        .count();
    if lost == processes.len() {
        return Err(ProfileError::NoProfile);
    }
    let root = camino::absolute_utf8(root).map_err(ProfileError::io(root))?;
    let mut sums = Sums::new(&root);
    for process in processes.values() {
        let mut scripts = Scripts::load(&process.maps)?;
        for path in process.chunks.values() {
            let bytes = fs::read(path).map_err(ProfileError::io(path))?;
            let profile: Profile<'_> = serde_json::from_slice(&bytes)
                .map_err(|error| ProfileError::bad(path, error.to_string()))?;
            sums.add(&profile, &mut scripts)
                .map_err(|message| ProfileError::bad(path, message))?;
        }
    }
    Ok(sums.finish(count(processes.len()), count(lost)))
}

/// Skips other names, such as `preload.cjs` or a `.tmp` that a kill left.
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
        let files = match kind {
            "cpuprofile" => &mut processes.entry(id.to_owned()).or_default().chunks,
            "maps.json" => &mut processes.entry(id.to_owned()).or_default().maps,
            _ => continue,
        };
        files.insert(n, entry.into_path());
    }
    Ok(processes)
}

/// Files are indexes into `Sums::files`, lines and columns from 1.
enum Frame {
    Function {
        file: u32,
        line: u32,
        column: u32,
        /// Sampled lines in the original file, with their ticks.
        lines: Vec<(u32, u32)>,
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
#[derive(Default)]
struct Sums {
    root: Utf8PathBuf,
    total: f64,
    local: f64,
    unmapped: f64,
    files: Vec<Utf8PathBuf>,
    file_ids: HashMap<String, u32>,
    functions: HashMap<(u32, u32, u32), Function>,
    modules: HashMap<u32, f64>,
}

impl Sums {
    fn new(root: &Utf8Path) -> Sums {
        Sums {
            root: maps::normalize(root),
            ..Sums::default()
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

        let frames: Vec<Frame> = nodes
            .iter()
            .map(|node| self.resolve(node, scripts))
            .collect();
        let mut counted = Vec::new();
        for (leaf, &us) in self_us.iter().enumerate().filter(|(_, us)| **us > 0.0) {
            self.total += us;
            let frame = &frames[leaf];
            if !matches!(frame, Frame::Other) {
                self.local += us;
            }
            match frame {
                Frame::Function {
                    file,
                    line,
                    column,
                    lines,
                } => {
                    let function = self.functions.entry((*file, *line, *column)).or_default();
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
                Frame::Module { file } => *self.modules.entry(*file).or_default() += us,
                Frame::Unmapped => self.unmapped += us,
                Frame::Other => {}
            }
            counted.clear();
            let mut at = Some(leaf);
            while let Some(node) = at {
                if let Frame::Function {
                    file, line, column, ..
                } = frames[node]
                {
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

    fn resolve(&mut self, node: &Node<'_>, scripts: &mut Scripts) -> Frame {
        let call = &node.call_frame;
        let script = scripts.get(&call.url);
        let (Some(file), Ok(line), Ok(column)) = (
            script.file.as_deref(),
            u32::try_from(call.line_number),
            u32::try_from(call.column_number),
        ) else {
            return Frame::Other;
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
                return self.module(module);
            };
            return self.function(source, line + 1, column + 1, node, |line| {
                let (in_file, line) = map.line(line.checked_sub(1)?)?;
                (in_file == source).then_some(line + 1)
            });
        }
        if file
            .extension()
            .is_some_and(|extension| TS_EXTENSIONS.contains(&extension))
        {
            return if self.is_local(file) {
                Frame::Unmapped
            } else {
                Frame::Other
            };
        }
        if top_level {
            return self.module(file);
        }
        self.function(file, line + 1, column + 1, node, Some)
    }

    /// `original` turns a line of the generated code, from 1, into one of the source.
    fn function(
        &mut self,
        file: &Utf8Path,
        line: u32,
        column: u32,
        node: &Node<'_>,
        original: impl Fn(u32) -> Option<u32>,
    ) -> Frame {
        let Some(file) = self.local_id(file) else {
            return Frame::Other;
        };
        let lines = node
            .position_ticks
            .iter()
            .filter_map(|tick| Some((original(tick.line)?, tick.ticks)))
            .collect();
        Frame::Function {
            file,
            line,
            column,
            lines,
        }
    }

    fn module(&mut self, file: &Utf8Path) -> Frame {
        self.local_id(file)
            .map_or(Frame::Other, |file| Frame::Module { file })
    }

    /// On Windows `src\Orders.js` and `src\orders.js`, as two `require`s may spell one file, get
    /// one id and the first spelling.
    fn local_id(&mut self, file: &Utf8Path) -> Option<u32> {
        let key = if cfg!(windows) {
            Cow::Owned(file.as_str().to_ascii_lowercase())
        } else {
            Cow::Borrowed(file.as_str())
        };
        if let Some(&id) = self.file_ids.get(&*key) {
            return Some(id);
        }
        if !self.is_local(file) {
            return None;
        }
        let id = count(self.files.len());
        self.files.push(file.to_owned());
        self.file_ids.insert(key.into_owned(), id);
        Some(id)
    }

    /// Under `root` and outside `node_modules`.
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

/// Windows ignores the case of file names.
fn same_name(a: &str, b: &str) -> bool {
    if cfg!(windows) {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}
