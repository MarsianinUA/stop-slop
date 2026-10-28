'use strict';
// The stand starts a Node service with the variables of `node_env` (`env.rs`):
// NODE_OPTIONS="--require <this file>", so it runs in every Node process and worker thread of
// the service; STOP_SLOP_PROFILE_DIR, the folder of the chunks; STOP_SLOP_PROFILE_MODE, `cpu`
// for CPU profiles or `coverage` for the calls of each function. What it writes is read by
// `read.rs`. The program must run as it would without it: no signal listeners, no
// `process.setSourceMapsEnabled` (it rewrites `Error.stack`), and no error of the preload
// reaches the program. The one exception: its `exit` listener counts toward the program's
// limit, so a program with exactly 10 `exit` listeners of its own gets a
// MaxListenersExceededWarning.

const dir = process.env.STOP_SLOP_PROFILE_DIR;
if (dir) {
  try {
    start(dir, process.env.STOP_SLOP_PROFILE_MODE === 'coverage');
  } catch {
    // Without a chunk, `read` counts the process as lost.
  }
}

function start(dir, coverage) {
  const fs = require('node:fs');
  const path = require('node:path');
  const inspector = require('node:inspector');
  const { fileURLToPath } = require('node:url');
  const { threadId } = require('node:worker_threads');

  const pid = threadId === 0 ? String(process.pid) : `${process.pid}-t${threadId}`;
  // Windows soon gives a freed pid to a new process: one that finds its pid taken by an earlier
  // process of the run counts on, `<pid>-r1`, `<pid>-r2`.
  let id = pid;
  for (let n = 1; !claim(`${id}.start.json`); n++) id = `${pid}-r${n}`;

  const session = new inspector.Session();
  session.connect();

  // jest compiles modules with `vm.compileFunction`, and Node's map cache never sees them;
  // V8 still reads the `sourceMappingURL` comment of every script it parses. jest compiles each
  // module again for every test file: the first compile of a URL is kept until a chunk writes
  // its map.
  const mapURLs = new Map();
  const seen = new Set();
  const dependency = /[\\/]node_modules[\\/]/;
  session.on('Debugger.scriptParsed', ({ params: { url, sourceMapURL } }) => {
    if (!sourceMapURL || seen.has(url) || mapURLs.has(url)) return;
    // Frames of dependencies are never local, so their maps would only take space.
    if (dependency.test(url)) return;
    mapURLs.set(url, sourceMapURL);
  });
  // Without the limit the debugger keeps the source of every script the program dropped, and
  // jest drops one per module and test file: 3000 compiles of 110 KB took 940 MB, not 280 MB.
  post('Debugger.enable', { maxScriptsCacheSize: 0 });
  // Otherwise every `debugger;` of the program pauses and resumes through this session:
  // 20,000 of them took 8 s instead of 5 ms. A debugger the user attaches still stops there.
  post('Debugger.setSkipAllPauses', { skip: true });

  post('Profiler.enable');
  if (coverage) {
    // A count per function, without the ranges of its blocks.
    post('Profiler.startPreciseCoverage', { callCount: true, detailed: false });
  } else {
    // V8 drops the inspector's CPU profiler when its last profile stops, and the next start
    // logs every function in the heap again: 130-150 ms with 60k functions. A `console.profile`
    // that is never ended keeps it alive, so a chunk's stop and start cost 0.2 ms.
    console.profile('stop-slop');
    post('Profiler.start');
  }

  const lineStarts = new Map();
  let chunk = 0;
  // A process killed hard (TerminateProcess on Windows, jest ending its workers) runs no
  // handler and loses what ran since its last chunk: up to 2 s, or a whole synchronous run
  // that kept the timer from firing.
  setInterval(flush, 2000).unref();
  process.on('exit', flush);

  function flush() {
    try {
      const { kind, value, urls } = coverage ? takeCoverage() : takeProfile();
      const fresh = new Set();
      const maps = {};
      for (const url of urls) {
        if (!url || seen.has(url) || fresh.has(url)) continue;
        fresh.add(url);
        const sourceMapURL = mapURLs.get(url);
        const map = sourceMapURL && mapOf(url, sourceMapURL);
        if (map) maps[url] = map;
      }
      const n = chunk++;
      // The maps go first: a chunk on disk always has the maps it needs.
      if (Object.keys(maps).length > 0) write(`${id}.${n}.maps.json`, maps);
      // Only after the write: when it fails, the next chunk writes these maps.
      for (const url of fresh) {
        seen.add(url);
        mapURLs.delete(url);
      }
      write(`${id}.${n}.${kind}`, value);
    } catch {
      // A full disk loses the chunk, not the program.
    }
  }

  function takeProfile() {
    const { profile } = post('Profiler.stop');
    post('Profiler.start');
    const urls = profile.nodes.map((node) => node.callFrame.url);
    return { kind: 'cpuprofile', value: profile, urls };
  }

  // V8 resets the counts on every take, so the chunks of a process add up. Scripts of
  // dependencies are never local, and a Nest app loads thousands of them.
  function takeCoverage() {
    const { result } = post('Profiler.takePreciseCoverage');
    const scripts = result.filter(
      (script) =>
        script.url.startsWith('file:') &&
        !dependency.test(script.url) &&
        !isSelf(script.url) &&
        script.functions.some((fn) => fn.ranges[0].count > 0) &&
        place(script),
    );
    return { kind: 'coverage.json', value: scripts, urls: scripts.map((script) => script.url) };
  }

  // Not against `pathToFileURL(__filename)`: it writes `~` as `%7E`, and V8's URL keeps it, as
  // in the Windows short name `RUNNER~1`.
  function isSelf(url) {
    try {
      return fileURLToPath(url) === __filename;
    } catch {
      // A vm script can be named any `file:` URL, even one with no path on this OS.
      return false;
    }
  }

  // The host has no copy of the generated script, so each function gets the line and column
  // from 0 of its start here, counted as V8 counts `callFrame.lineNumber`. A script whose text
  // is gone gives `false` and leaves the chunk.
  function place(script) {
    let starts = lineStarts.get(script.scriptId);
    if (!starts) {
      try {
        const { scriptSource } = post('Debugger.getScriptSource', { scriptId: script.scriptId });
        starts = [0];
        for (const end of scriptSource.matchAll(/\r\n?|[\n\u2028\u2029]/g)) {
          starts.push(end.index + end[0].length);
        }
      } catch {
        return false;
      }
      // A script's text never changes, and a script with new calls comes back in later chunks.
      lineStarts.set(script.scriptId, starts);
    }
    for (const fn of script.functions) {
      const offset = fn.ranges[0].startOffset;
      let low = 0;
      let high = starts.length - 1;
      while (low < high) {
        const middle = (low + high + 1) >> 1;
        if (starts[middle] <= offset) low = middle;
        else high = middle - 1;
      }
      fn.line = low;
      fn.column = offset - starts[low];
    }
    return true;
  }

  // The host has none of the guest's files, so the map goes whole, with `base`, the path its
  // relative `sources` start from: the script's or the map file's. `sourcesContent` would only
  // take space.
  function mapOf(url, sourceMapURL) {
    try {
      const inline = /^data:application\/json[^,]*?(;base64)?,(.*)$/s.exec(sourceMapURL);
      let text;
      let base;
      if (inline) {
        text = inline[1]
          ? Buffer.from(inline[2], 'base64').toString('utf8')
          : decodeURIComponent(inline[2]);
        base = fileURLToPath(url);
      } else {
        const file = new URL(sourceMapURL, url);
        if (file.protocol !== 'file:') return undefined;
        base = fileURLToPath(file);
        text = fs.readFileSync(base, 'utf8');
      }
      const map = JSON.parse(text);
      delete map.sourcesContent;
      return { map, base };
    } catch {
      return undefined;
    }
  }

  // With the session in this thread, `post` runs the callback before it returns.
  function post(method, params) {
    let failure;
    let result;
    session.post(method, params, (error, value) => {
      failure = error;
      result = value;
    });
    if (failure) throw failure;
    return result;
  }

  // `read` never opens a start mark, so it is created in place, which only one process can do.
  function claim(name) {
    try {
      fs.writeFileSync(path.join(dir, name), '{}', { flag: 'wx' });
      return true;
    } catch (error) {
      if (error.code === 'EEXIST') return false;
      throw error;
    }
  }

  // A process killed while writing leaves only the `.tmp` file, which `read` skips.
  function write(name, value) {
    const file = path.join(dir, name);
    fs.writeFileSync(`${file}.tmp`, JSON.stringify(value));
    fs.renameSync(`${file}.tmp`, file);
  }
}
