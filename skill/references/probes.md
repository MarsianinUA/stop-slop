# probes

A probe is a command from the recipe that runs inside the VM of a service and reads the state below the app: rows of a table, keys of a store, files on a volume. The stand compares its output between "before" and the head like a file of the workload. The stand knows no database: the examples below are recipes to adapt.

- `x-stand.probes.<name>` is `{in, at, run, key, ignore}`. The name follows the rules of a service name; the output is `probe.<name>.jsonl` in `/stand/out`.
  - `in` is a long-running service: the probe runs in its first replica, which already runs.
  - `at` is `after-setup` (once per version, in a service that runs in setup: one a setup service waits for), `after-workload` (after every measured repeat and each pass of the second pass) or `on-failure` (before the stop of a failed setup or repeat; its output path is in `failure.probes`).
  - `run` is the argv, a list. `$` is written `$$`, as everywhere in Compose: `["sh", "-c", "echo $$HOME"]`.
  - `key` (optional) is a JSON Pointer: records pair by its value instead of by their position. `ignore` (optional) lists JSON Pointers to leave out of the comparison, such as times.
- The probe runs under 30 seconds, as the image's user, outside the measured window. Its sockets reach only the loopback of its own VM: it reads its own service, never a neighbour.
- Each line of stdout is a record. A line that is not JSON compares as `{"line": "<text>"}`.
- A probe that exits non-zero or hits its deadline is the flag `probe-failed`; one with no record anywhere, or whose `ignore` takes every field, is `dead-probe`: it could never show a change.

## Postgres: the rows a run wrote

A logical slot with `test_decoding` lists every change since setup: each row inserted, updated or deleted, in order.

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
      run: ["psql", "-U", "postgres", "-At", "-c", "select pg_create_logical_replication_slot('stand', 'test_decoding')"]
    changes:
      in: db
      at: after-workload
      run: ["psql", "-U", "postgres", "-At", "-c", "checkpoint; select json_build_object('xid', xid, 'data', data) from pg_logical_slot_peek_changes('stand', null, null)"]
      ignore: [/xid]
```

- The slot comes after setup, so it lists only what the workload did; `after-setup` runs before the setup snapshot, so every repeat starts with it.
- `ALTER TABLE <t> REPLICA IDENTITY FULL` in a migration puts the old row into each update and delete.
- `checkpoint` before the peek: without it the last changes may not be decoded yet.

A plainer probe: the rows of a table, paired by id, without their times.

```yaml
    orders:
      in: db
      at: after-workload
      run: ["psql", "-U", "postgres", "-At", "-c", "select row_to_json(o) from orders o order by id"]
      key: /id
      ignore: [/updated_at]
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
- The volume is an ext4 disk of the VM, so names, case and links are Linux's.
