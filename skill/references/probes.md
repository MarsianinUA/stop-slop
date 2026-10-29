# probes

A probe is a command from the recipe that runs inside the VM of a service and reads the state below the app: rows of a table, keys of a store, files on a volume. The stand compares its output between "before" and the head like a file of the workload. The stand knows no database: the examples below are recipes to adapt.

- `x-stand.probes.<name>` is `{in, at, run, key, ignore}`. The name follows the rules of a service name; the output is `probe.<name>.jsonl` in `/stand/out`.
  - `in` is a long-running service: the probe runs in its first replica, which already runs.
  - `at` is `after-setup` (once per version, in a service that runs in setup: one a setup service waits for), `after-workload` (after every measured repeat and each pass of the second pass) or `on-failure` (before the stop of a failed setup or repeat; its output path is in `failure.probes`).
  - `run` is the argv, a list. `$` is written `$$`, as everywhere in Compose: `["sh", "-c", "echo $$HOME"]`. A probe longer than one command is a script of the harness, mounted read-only into its service, such as `./probes:/probes:ro`, and run as `["sh", "/probes/<script>.sh"]`: a script needs no `$$`. Give it LF line ends: `sh` in the guest reads a CR as part of the line.
  - `key` (optional) is a JSON Pointer: records pair by its value instead of by their position. `ignore` (optional) lists JSON Pointers to leave out of the comparison, such as times.
- The probe runs under 30 seconds, as the image's user, outside the measured window. Its sockets reach only the loopback of its own VM: it reads its own service, never a neighbour.
- Each line of stdout is a record. A line that is not JSON compares as `{"line": "<text>"}`.
- The stand compares records field by field, along JSON Pointers. A value that changes every run, such as a time or a random id, goes into `ignore`, or the probe leaves it out: otherwise the stand takes its whole field path for noise in every record of the file, and the data at that path no longer compares. Inside a field that also carries data, such as a line of text, it hides that data too. Records without `key` pair by position: print them in a fixed order.
- A probe that exits non-zero or hits its deadline is the flag `probe-failed`; one with no record anywhere, or whose `ignore` takes every field, is `dead-probe`: it could never show a change.

## Postgres: what a run wrote

A logical slot with `test_decoding` lists every change since setup in commit order: each row inserted, updated or deleted.

```yaml
services:
  db:
    image: postgres:17
    command: ["postgres", "-c", "wal_level=logical"]
x-stand:
  probes:
    slot:
      in: db
      at: after-setup
      run: ["psql", "-U", "postgres", "-At", "-c", "select slot_name from pg_create_logical_replication_slot('stand', 'test_decoding')"]
    writes:
      in: db
      at: after-workload
      run: ["psql", "-U", "postgres", "-Atq", "-c", "checkpoint", "-c", "select json_build_object('table', m[1], 'op', coalesce(m[2], c.data)) from pg_logical_slot_peek_changes('stand', null, null, 'include-xids', '0', 'skip-empty-xacts', '1') c, regexp_match(c.data, '^table ([^:]+): ([A-Z]+):') m"]
```

- The slot comes after setup, so it lists only what the workload did; `after-setup` runs before the setup snapshot, so every repeat starts with it. `select slot_name` prints only the name: where the slot starts differs between the versions.
- `checkpoint` before the peek: without it the last changes may not be decoded yet.
- A change is `{table, op}`, and `BEGIN` and `COMMIT` mark the transactions: a write more or less, a write to another table or a transaction split in two is a diff. The values stay out: a time or a random id in a row would turn the field into noise in every record. The rows of a table compare in the probe below.
- To see the values here anyway, print `data` itself, for tables whose rows hold no such value. `ALTER TABLE <t> REPLICA IDENTITY FULL` in a migration puts the old row into each update and delete.
- A workload that writes in parallel commits in another order each repeat, and the changes turn into noise. Count them instead: `select json_build_object('table', m[1], 'op', m[2], 'count', count(*)) from pg_logical_slot_peek_changes('stand', null, null) c, regexp_match(c.data, '^table ([^:]+): ([A-Z]+):') m where m is not null group by m[1], m[2] order by m[1], m[2]`.

The rows of a table, paired by id, without their times:

```yaml
    orders:
      in: db
      at: after-workload
      run: ["psql", "-U", "postgres", "-At", "-c", "select row_to_json(o) from orders o order by id"]
      key: /id
      ignore: [/updated_at]
```

## Postgres: the statements a run sent

`pg_stat_statements` counts each statement by its normalized text: a query sent more often, or a new one, is a diff of its record.

```yaml
services:
  db:
    image: postgres:17
    command: ["postgres", "-c", "shared_preload_libraries=pg_stat_statements"]
x-stand:
  probes:
    statements-reset:
      in: db
      at: after-setup
      run: ["psql", "-U", "postgres", "-Atq", "-c", "create extension if not exists pg_stat_statements", "-c", "select 'reset' from pg_stat_statements_reset()"]
    statements:
      in: db
      at: after-workload
      run: ["psql", "-U", "postgres", "-At", "-c", "select json_build_object('query', query, 'calls', calls, 'rows', rows) from pg_stat_statements where query not like '%pg_stat_statements%' order by query"]
      key: /query
```

- The counts outlive the clean stop before the setup snapshot, so the reset runs at the end of setup, as an `after-setup` probe: every repeat starts from the same, nearly empty counts. It prints `reset`, not the time `pg_stat_statements_reset()` returns.
- The probes' own statements count too, the same on both versions; the filter takes out the ones that read the counts. When the app has a role of its own, `where userid = 'app'::regrole` keeps only its statements.
- With the slot as well, both settings go into one `command`: `["postgres", "-c", "wal_level=logical", "-c", "shared_preload_libraries=pg_stat_statements"]`.

## Redis: the writes a run made

The append-only file of Redis logs every write with its values. Its tail after setup is what the workload wrote, one record per command.

```yaml
services:
  cache:
    image: redis:7
    command: ["redis-server", "--appendonly", "yes", "--auto-aof-rewrite-percentage", "0"]
    volumes: ["cachedata:/data", "./probes:/probes:ro"]
    expose: [6379]
    healthcheck: { test: ["CMD", "redis-cli", "ping"], interval: 1s, retries: 30 }
x-stand:
  probes:
    aof-start:
      in: cache
      at: after-setup
      run: ["sh", "/probes/aof.sh", "start"]
    aof:
      in: cache
      at: after-workload
      run: ["sh", "/probes/aof.sh"]
```

- `after-setup` runs only in a service setup waits for: a setup service, such as `migrate`, waits for `cache` with `service_healthy`.
- `aof-start` notes on the volume which file the tail is in and where it starts, so every repeat starts with the note. `--auto-aof-rewrite-percentage 0` keeps Redis from rewriting the log into a new file; a rewrite since setup fails the probe (`probe-failed`).
- Redis answers a write before it is in the file: the script waits until `INFO persistence` shows the buffer written and synced, before the note and before the tail.
- Redis logs `EXPIRE` as `PEXPIREAT` and `SET .. EX` as `SET .. PXAT`, with an absolute time, new each run: the script prints `<time>` in its place. A key or value that is random each run, such as a session id, would turn its field into noise the same way: map it too.

`probes/aof.sh`, beside the stand file:

```sh
#!/bin/sh
# `aof.sh start` notes where the tail of the AOF starts; `aof.sh` prints that tail.
set -eu
dir=/data/appendonlydir
note=/data/stand-aof-start
until redis-cli info persistence | tr -d '\r' | grep -qx 'aof_buffer_length:0' &&
  redis-cli info persistence | tr -d '\r' | grep -qx 'aof_pending_bio_fsync:0'; do
  sleep 0.1
done
incr=$(awk '$NF == "i" { file = $2 } END { print file }' "$dir"/*.manifest)
if [ "${1:-}" = start ]; then
  size=$(wc -c < "$dir/$incr")
  echo "$incr $size" > "$note"
  printf '{"file":"%s","start":%s}\n' "$incr" "$size"
  exit 0
fi
read -r start_file start < "$note"
if [ "$incr" != "$start_file" ]; then
  echo "Redis rewrote its AOF after setup: the tail is gone" >&2
  exit 1
fi
tail -c +"$((start + 1))" "$dir/$incr" | tr -d '\r' | awk -f /probes/resp.awk
```

`probes/resp.awk` turns the commands of the log into JSON:

```awk
# A command is a line *<count>, then <count> times a line $<length> and a line with the
# argument. An argument with a line break in it breaks this.
state == 0 { count = substr($0, 2) + 0; n = 0; state = 1; next }
state == 1 { state = 2; next }
{
  arg[n++] = $0
  if (n < count) { state = 1; next }
  state = 0
  print record()
}

function record(  cmd, out, i, value) {
  cmd = toupper(arg[0])
  out = "{\"cmd\":\"" quote(cmd) "\",\"args\":["
  for (i = 1; i < n; i++) {
    value = arg[i]
    if ((cmd == "PEXPIREAT" && i == 2) || toupper(arg[i - 1]) == "PXAT") value = "<time>"
    out = out (i > 1 ? "," : "") "\"" quote(value) "\""
  }
  return out "]}"
}

function quote(text,  out, i, c) {
  out = ""
  for (i = 1; i <= length(text); i++) {
    c = substr(text, i, 1)
    if (c == "\"" || c == "\\") out = out "\\"
    out = out c
  }
  return out
}
```

## Redis: calls, keys and memory

```yaml
    redis-calls:
      in: cache
      at: after-workload
      run: ["sh", "/probes/redis-calls.sh"]
      key: /cmd
    redis-keys:
      in: cache
      at: after-workload
      run: ["sh", "/probes/redis-keys.sh"]
      key: /prefix
```

`probes/redis-calls.sh`: the calls of each command since the repeat started Redis. The `ping` of the healthcheck and the `info` of the probes vary with timing, so they stay out.

```sh
#!/bin/sh
redis-cli info commandstats | tr -d '\r' |
  sed -n 's/^cmdstat_\([^:]*\):calls=\([0-9]*\),.*/{"cmd":"\1","calls":\2}/p' |
  grep -Ev '"cmd":"(ping|info)"'
```

`probes/redis-keys.sh`: the keys that never expire, counted by the prefix before their first `:`. A change that adds keys without a TTL grows the store for good. The memory Redis takes is the stand's own measure `<service>.memory`; `used_memory` moves with every allocation, so the probe leaves it out.

```sh
#!/bin/sh
redis-cli eval "
local counts = {}
for _, key in ipairs(redis.call('keys', '*')) do
  if redis.call('ttl', key) == -1 then
    local prefix = string.match(key, '^[^:]*')
    counts[prefix] = (counts[prefix] or 0) + 1
  end
end
local out = {}
for prefix, n in pairs(counts) do out[#out + 1] = prefix .. ' ' .. n end
table.sort(out)
return out" 0 | while read -r prefix keys; do
  [ -n "$keys" ] && printf '{"prefix":"%s","keys":%s}\n' "$prefix" "$keys"
done
```

## Queues: BullMQ and JetStream

What a queue holds after the workload: jobs waiting, delayed, failed and done.

```yaml
    queues:
      in: cache
      at: after-workload
      run: ["sh", "/probes/queues.sh"]
      key: /queue
    streams:
      in: nats
      at: after-workload
      run: ["sh", "/probes/streams.sh"]
```

`probes/queues.sh`: the jobs of each BullMQ queue by state, one record per queue, the app's dead-letter queue among them.

```sh
#!/bin/sh
redis-cli --scan --pattern 'bull:*:meta' | sed 's/^bull:\(.*\):meta$/\1/' | sort |
  while read -r queue; do
    printf '{"queue":"%s","wait":%s,"active":%s,"delayed":%s,"failed":%s,"completed":%s}\n' "$queue" \
      "$(redis-cli llen "bull:$queue:wait")" "$(redis-cli llen "bull:$queue:active")" \
      "$(redis-cli zcard "bull:$queue:delayed")" "$(redis-cli zcard "bull:$queue:failed")" \
      "$(redis-cli zcard "bull:$queue:completed")"
  done
```

`probes/streams.sh`: the messages of each JetStream stream and the pending and redelivered counts of each consumer, one field a record, in the order of the answer. Its times and ids change every run, so the script keeps only these lines of it.

```sh
#!/bin/sh
wget -qO- 'http://127.0.0.1:8222/jsz?consumers=1' |
  sed -nE 's/^ *("(name|messages|num_pending|num_ack_pending|num_redelivered)": [^,]+),?$/{\1}/p'
```

- The NATS service runs `nats:2-alpine` with `command: ["nats-server", "-js", "-sd", "/data", "-m", "8222"]`, a volume on `/data` and `./probes:/probes:ro`. The plain `nats` image has no shell, nor the `id`, `tar` and `du` the stand needs.
- A probe reads a still queue only if the workload waited for it, so end the workload with a drain barrier: it waits until no job waits, runs or is delayed. The jobs then also run inside the measured window. The barrier keeps one connection for the whole wait: a new one per poll would run into the limit of new connections (`guide recipe`).

```js
// The end of load.mjs: wait until the queue `emails` holds no job waiting, running or delayed.
import { once } from "node:events";
import { createConnection } from "node:net";

const redis = createConnection(6379, "cache");
await once(redis, "connect");
let replies = "";
redis.setEncoding("utf8").on("data", (chunk) => (replies += chunk));
for (;;) {
  replies = "";
  redis.write("LLEN bull:emails:wait\r\nLLEN bull:emails:active\r\nZCARD bull:emails:delayed\r\n");
  while (replies.split("\r\n").length < 4) await once(redis, "data");
  if (replies.split("\r\n").slice(0, 3).every((reply) => reply === ":0")) break;
  await new Promise((resolve) => setTimeout(resolve, 200));
}
redis.end();
```

## Files on a volume: a manifest

Path, size and sha256 of every file under a folder, one JSON record each, paired by path.

```yaml
    files:
      in: store
      at: after-workload
      run: ["sh", "-c", "cd /data && find . -type f | sort | while read -r f; do printf '{\"path\":\"%s\",\"size\":%s,\"sha256\":\"%s\"}\\n' \"$$f\" \"$$(stat -c %s \"$$f\")\" \"$$(sha256sum \"$$f\" | cut -d' ' -f1)\"; done"]
      key: /path
```

- Busybox `stat` and `sha256sum` do this too, in alpine images.
- A file whose content changes every run, such as a log, stays out of the `find`: its `sha256` would make `/sha256` noise for every file.
- The volume is an ext4 disk of the VM, so names, case and links are Linux's.

## What the workload records

The workload's own files in `/stand/out` compare the same way: what the app answers is behaviour too.

```js
// Parts of a load.mjs: the refusals, a race, the contract.
import { appendFileSync } from "node:fs";

const base = process.argv[2];
const write = (file, record) => appendFileSync(`/stand/out/${file}`, `${JSON.stringify(record)}\n`);
const parse = (text) => {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
};

// Read every body: Node's fetch reuses a connection only after that.
async function call(path, init) {
  const answer = await fetch(`${base}${path}`, init);
  return { status: answer.status, body: parse(await answer.text()) };
}

write("auth.jsonl", { case: "no key", ...(await call("/orders/1")) });
const bob = { headers: { authorization: "Bearer key-of-bob" } };
write("auth.jsonl", { case: "another customer's key", ...(await call("/orders/1", bob)) });

// One order sent five times at once, as a double click does; sorted, so their order is no noise.
const same = { method: "POST", headers: { "idempotency-key": "order-1" } };
const answers = await Promise.all(Array.from({ length: 5 }, () => call("/orders", same)));
write("race.jsonl", { statuses: answers.map((answer) => answer.status).sort() });

// The OpenAPI document as one record: a changed route, field or type is a diff of its own.
write("openapi.jsonl", (await call("/docs-json")).body);
```

- The answers of the routes the branch reaches, a record `{route, status, body}` each, show a changed answer field by field. A workload file has no `ignore`: the workload leaves out a time or a random id of a body before it writes the record.
- `/docs-json` is where NestJS serves the document with `SwaggerModule`; another app has its own path.
- A job on a schedule, a cron or an interval, runs inside a repeat only with a short schedule. When the app takes its schedule from config, give it one such as every 5 seconds in `environment`, and let the workload wait for the job to run.
- Guests run in UTC, where local and UTC dates agree. `TZ: Pacific/Kiritimati` (UTC+14) in the app's `environment` makes a change that mixes the two show in the records.
