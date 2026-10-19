'use strict';
// `stop-slop profile` writes this file into the profile folder and starts the command with
// NODE_OPTIONS="--require <this file>", so it runs in every Node process and worker thread
// the command starts. What it writes is read by `read.rs`. The program must run as it would
// without it: no signal listeners, no `process.setSourceMapsEnabled` (it rewrites
// `Error.stack`), and no error of the preload reaches the program.

const dir = process.env.STOP_SLOP_PROFILE_DIR;
if (dir) {
  try {
    start(dir);
  } catch {
    // Without a chunk, `read` counts the process as lost.
  }
}

function start(dir) {
  const fs = require('node:fs');
  const path = require('node:path');
  const inspector = require('node:inspector');
  const { fileURLToPath, pathToFileURL } = require('node:url');
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
  session.on('Debugger.scriptParsed', ({ params: { url, sourceMapURL } }) => {
    if (!sourceMapURL || seen.has(url) || mapURLs.has(url)) return;
    // Frames of dependencies are never local, so their maps would only take space.
    if (/[\\/]node_modules[\\/]/.test(url)) return;
    mapURLs.set(url, sourceMapURL);
  });
  // Without the limit the debugger keeps the source of every script the program dropped, and
  // jest drops one per module and test file: 3000 compiles of 110 KB took 940 MB, not 280 MB.
  post('Debugger.enable', { maxScriptsCacheSize: 0 });

  post('Profiler.enable');
  // V8 drops the inspector's CPU profiler when its last profile stops, and the next start logs
  // every function in the heap again: 130-150 ms with 60k functions. A `console.profile` that
  // is never ended keeps it alive, so a chunk's stop and start cost 0.2 ms.
  console.profile('stop-slop');
  post('Profiler.start');

  let chunk = 0;
  // A process killed hard (TerminateProcess on Windows, jest ending its workers) runs no
  // handler: chunks bound the loss to the last 2 s.
  setInterval(() => flush(true), 2000).unref();
  process.on('exit', () => flush(false));

  function flush(again) {
    try {
      const { profile } = post('Profiler.stop');
      if (again) post('Profiler.start');
      const fresh = new Set();
      const maps = {};
      for (const { callFrame: { url } } of profile.nodes) {
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
      write(`${id}.${n}.cpuprofile`, profile);
    } catch {
      // A full disk loses the chunk, not the program.
    }
  }

  function mapOf(url, sourceMapURL) {
    try {
      const inline = /^data:application\/json[^,]*?(;base64)?,(.*)$/s.exec(sourceMapURL);
      if (inline) {
        const text = inline[1]
          ? Buffer.from(inline[2], 'base64').toString('utf8')
          : decodeURIComponent(inline[2]);
        const map = JSON.parse(text);
        delete map.sourcesContent;
        return { map };
      }
      // A script compiled through `vm` may be named by a path instead of a URL.
      const base = path.isAbsolute(url) ? pathToFileURL(url) : url;
      const file = new URL(sourceMapURL, base);
      return file.protocol === 'file:' ? { file: fileURLToPath(file) } : undefined;
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
