use std::borrow::Cow;
use std::collections::HashMap;
use std::fs;

use camino::{Utf8Path, Utf8PathBuf};
use serde::Deserialize;

use crate::maps::{Script, Scripts};
use crate::read::{is_typescript, list};
use crate::{Called, Coverage, PathMap, ProfileError};

/// V8's `Profiler.ScriptCoverage`.
#[derive(Deserialize)]
struct ScriptCoverage<'a> {
    #[serde(borrow)]
    url: Cow<'a, str>,
    #[serde(borrow)]
    functions: Vec<FunctionCoverage<'a>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FunctionCoverage<'a> {
    #[serde(borrow)]
    function_name: Cow<'a, str>,
    /// The first is the function's own: `detailed: false` gives no ranges of blocks.
    ranges: Vec<CoverageRange>,
    /// From 0, in the generated script: the preload adds them.
    line: u32,
    column: u32,
}

#[derive(Deserialize)]
struct CoverageRange {
    count: u64,
}

/// Sums the calls in the coverage chunks the preload left in `dir` by function of the original
/// sources.
pub fn read_coverage(dir: &Utf8Path, paths: &PathMap) -> Result<Coverage, ProfileError> {
    let processes = list(dir, "coverage.json")?;
    if processes.values().all(|process| process.chunks.is_empty()) {
        return Err(ProfileError::NoProfile);
    }
    let mut calls = Calls::default();
    for process in processes.values() {
        let mut scripts = Scripts::load(&process.maps)?;
        for path in process.chunks.values() {
            let bytes = fs::read(path).map_err(ProfileError::io(path))?;
            let chunk: Vec<ScriptCoverage<'_>> = serde_json::from_slice(&bytes)
                .map_err(|error| ProfileError::bad(path, error.to_string()))?;
            calls
                .add(&chunk, &mut scripts, paths)
                .map_err(|message| ProfileError::bad(path, message))?;
        }
    }
    Ok(calls.finish())
}

/// By file, line and column from 1.
#[derive(Default)]
struct Calls {
    functions: HashMap<(Utf8PathBuf, u32, u32), Called>,
}

impl Calls {
    fn add(
        &mut self,
        chunk: &[ScriptCoverage<'_>],
        scripts: &mut Scripts,
        paths: &PathMap,
    ) -> Result<(), String> {
        for covered in chunk {
            let script = scripts.get(&covered.url);
            for function in &covered.functions {
                let range = function
                    .ranges
                    .first()
                    .ok_or_else(|| format!("{}: a function without ranges", covered.url))?;
                if range.count == 0 || function.is_top_level() {
                    continue;
                }
                let Some((file, line, column)) = origin(script, function.line, function.column)
                else {
                    continue;
                };
                let Some(file) = paths.local(file) else {
                    continue;
                };
                let called = self
                    .functions
                    .entry((file, line + 1, column + 1))
                    .or_insert_with_key(|(file, line, column)| Called {
                        file: file.clone(),
                        line: *line,
                        column: *column,
                        name: function.function_name.to_string(),
                        calls: 0,
                    });
                called.calls = called.calls.saturating_add(range.count);
            }
        }
        Ok(())
    }

    fn finish(self) -> Coverage {
        let mut called: Vec<Called> = self.functions.into_values().collect();
        called.sort_by(|a, b| (&a.file, a.line, a.column).cmp(&(&b.file, b.line, b.column)));
        Coverage { called }
    }
}

impl FunctionCoverage<'_> {
    /// V8 lists the top-level code of a script as an anonymous function at 0:0; a declaration
    /// there has a name.
    fn is_top_level(&self) -> bool {
        self.function_name.is_empty() && (self.line, self.column) == (0, 0)
    }
}

/// The original file, line and column (from 0) of a function start at generated `line`:`column`;
/// `None` for code the map has no original for, such as tsc's `__decorate`, or a `.ts` file
/// without a map.
fn origin(script: &Script, line: u32, column: u32) -> Option<(&str, u32, u32)> {
    let file = script.file.as_deref()?;
    if let Some(map) = &script.map
        && map.module().is_some()
    {
        return map.find(line, column);
    }
    (!is_typescript(file)).then_some((file, line, column))
}
