`stop-slop-stand` runs your app in microsandbox VMs from a Compose file, measures it, and prints one line of JSON with only what needs a look. It measures only your own branch.

## recipe

The recipe is the folder `<git common>/info/stop-slop-stand/`: the stand file `compose.yaml` and the harness beside it. It lives outside the repository, so nothing of it lands in the diff, and both versions mount the same harness.

- Services. Each has an `image` and `deploy.resources.limits.cpus` and `.memory`. A key the stand does not know is exit 2 with its path, `build` and `ports` among them: name a built image, and reach a service by its name and `expose`.
  - A service others wait for with `service_completed_successfully` is a setup service: it runs once per version, before the repeats. `x-stand.workload: <service>` names the workload: its exit ends a repeat, and nothing waits for it. The rest are long-running. All that wait for one service wait the same way: it is setup or long-running, not both.
  - A setup service and the workload run one replica.
  - A service another one calls needs `expose` (TCP): the stand puts every service name in each guest's hosts, and only exposed ports answer.
  - A guest has no outside network unless its service says `x-stand: { network: public }`. Give it only to what downloads, such as the prepare service.
  - An outside API the app calls, such as payments or mail, becomes a stub: a long-running service of the harness that answers as the real one does, the same way every repeat. The app finds it by a name in `environment`; the stub at the end of this topic also tells a probe what the app sent.
  - `service_healthy` needs a `healthcheck`. A service name has no `.` and is not `stand` or `volume`.
  - Each variable of `environment` has its value in the file: the stand takes none from the host. The variables of the `init` template, such as `DATABASE_URL`, are examples: keep the ones this app reads.
- Mounts, in the short form `source:target` or `source:target:ro`.
  - `${STAND_REPO}:/w/app` mounts the measured version's tree on its own disk, the version disk at `/w`, whole and writable: for a subfolder, set `working_dir`.
  - The harness: a folder of the recipe, such as `.` or `./load`, with `:ro`. A harness file goes into a folder, and the bind takes the folder. The folder exists inside the recipe folder, with no link on the way.
  - A named volume, listed in the top-level `volumes`, is the disk of the one service that mounts it, without `:ro`. That service runs one replica and no `${STAND_REPO}`.
  - `${STAND_REPO}` is the only substitution, and only as that source; a host path is refused. `$` in any string is `$$`, as in Compose.
  - `/stand/` and `/w/` are the stand's: no target under them but `/w/app`, no target `/`, and one mount per target.
- The prepare service: the setup service with `${STAND_REPO}` that every other `${STAND_REPO}` service waits for. It installs and builds on the version disk, nothing else. Its snapshot then seeds every other service with `${STAND_REPO}` in every repeat.
  - It mounts only `${STAND_REPO}`, and every other service with `${STAND_REPO}` uses its `image`, written the same: a VM from a snapshot keeps the snapshot's image.
  - Migrations and data go in a setup service of their own after it, as `migrate` in the template.
  - The package manager's store and cache go on `/w`, outside `/w/app`, never in a bind, where an install takes minutes instead of seconds: `npm ci --cache /w/.npm`, `pnpm install --store-dir /w/.pnpm-store`. `npm ci` needs a lockfile: without one, `npm install --cache /w/.npm`. pnpm 10 skips the build scripts of dependencies: allow them in the prepare command. Native modules build in the guest, never come from the host.
  - Keep the guest's root disk clean, with no global installs or caches: the snapshot keeps every block of it.
- The stand runs `id`, `tar` and `du` in the guests. An image without them, such as a distroless one, cannot run in it.
- `x-stand`: `workload`, `repeat` (3), `setup_timeout` (`10m`), `timeout` per repeat (`5m`), `probes` (`guide probes`), `expect` (`guide expect`), `fault` and `second_pass`. A service's `x-stand` takes `profile: node` for hot functions and coverage, on the services that serve the workload rather than on setup services, and `network: public`. `stop_signal` and `stop_grace_period` (`10s`) work as in Compose; every repeat ends with that clean stop.
- The workload writes `*.jsonl` into `/stand/out`. With `--before`, each file is compared record by record with the same file of "before"; fields that differ between repeats of "before" are noise.
- A value that changes every run, such as a time or a random id, stays out of a record, or goes into `ignore` of its probe (`guide probes`): otherwise the stand takes its whole field path for noise in every record of the file, and the data at that path no longer compares.
- Each `*.jsonl` of `/stand/out`, a probe's output too, is a plain file of at most 64 MiB: a link, a FIFO, a folder or a bigger file there is exit 2 with its path.
- The stand's network drops new connections to a service after about 256 within 10 seconds (`network-limit-hit`). Every caller counts, the app's own clients too, and so does each request that sends `Connection: close`. Keep connections open: keep-alive in the workload, a pool in the app (`pg.Pool`, not a client per request), and fewer than 25 new connections a second per service. Node's `fetch` reuses a connection only after its answer's body is read: read every body.
- Size the workload so that one repeat takes a few seconds before the change, and the measures you declare move more than their spread; one `run --json` before the edits gives the scale. Each commit waits for the disk: 10,000 single-row commits take about 33 seconds in a guest on Windows, so a repeat also stays well within `timeout`.
- `x-stand.fault: { target: "db:5432", mode: down }` cuts the target: new connections are refused and open ones dropped. `mode: slow` with `delay: 2s` holds each connection instead. The target is a long-running service with that port in `expose`, and a dependency of the app, such as its database: the fault reaches every VM of the repeat but the workload, so a target only the workload calls changes nothing.
- `x-stand.second_pass: true` runs the workload a second time on the state the first pass left, as a retry would; `expect.unchanged` names what must hold (`guide expect`).
- With `--before` a changed, new or deleted file of the recipe is exit 2 that names it. Put it back, or seal again: `run --before <..> --reseal --reason "<why>"`. The journal of the seal in the line shows each reseal.

A stub of a payment API. The app gets `PAYMENTS_URL: "http://payments:8080"` in its `environment` and waits for `payments` with `service_healthy`:

```yaml
services:
  payments:
    image: node:22-slim
    working_dir: /harness
    volumes: [".:/harness:ro"]
    command: ["node", "stub.mjs"]
    expose: [8080]
    healthcheck: { test: ["CMD", "node", "-e", "fetch('http://127.0.0.1:8080/health').then(r => process.exit(r.ok ? 0 : 1), () => process.exit(1))"], interval: 1s, retries: 30 }
    deploy: { resources: { limits: { cpus: "1", memory: 128m } } }
x-stand:
  probes:
    payments-sent:
      in: payments
      at: after-workload
      run: ["node", "-e", "fetch('http://127.0.0.1:8080/sent').then(r => r.text()).then(t => process.stdout.write(t))"]
```

`stub.mjs` beside the stand file answers every request with the same body and keeps what it got, which `GET /sent` prints one record per line:

```js
import { createServer } from "node:http";

const sent = [];
const parse = (text) => {
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
};

createServer(async (request, response) => {
  let body = "";
  for await (const chunk of request) body += chunk;
  if (request.url === "/health") return response.end("ok");
  if (request.url === "/sent") return response.end(sent.map((it) => `${JSON.stringify(it)}\n`).join(""));
  sent.push({ method: request.method, url: request.url, body: body ? parse(body) : null });
  response.setHeader("content-type", "application/json");
  response.end(JSON.stringify({ id: "pay_1", status: "succeeded" }));
}).listen(8080);
```

## expect

`x-stand.expect` declares the cost of the change before the first `run --before` of the session. The stand grades each measure against it (`grade`, `guide line`); a cost declared after the measure would explain nothing.

- `cost` is a list of `{measure, change, reason}`, one entry per measure. `measure` is a name of the line, such as `stand.wall`, `api.memory`, `api:3000.connections` or `volume:pgdata.bytes`. A measure without an entry is `none`.
- `change` is `none`, `+X%`, `-X%`, `down` or `unknown`, without quotes, X a whole percent from 5. `none` means the measure stays within its noise, up to about 5%. `unknown` asks the user: the measure goes to `needs` with its reason.
- `reason` says why, in a few words: "one more index lookup per order".
- A bound wider than `max_tolerance` of `stand.toml` (25 by default) is refused, in `cost` and in `accept`: such a shift needs the user.
- A `surprise` or an `exceeded` comes with an `accept` command in `next`, its bound the shift rounded up to 5%. Accept only a shift you can explain: `stop-slop-stand accept <measure> --allow <bound> --reason "<why>"`.
  - `--allow` takes the grammar of `change` but `unknown`, and the measure must be in the last passed run of this work tree.
  - An acceptance holds for this head tree only: the line shows it as `postHoc`, and as `stale` after the next edit.
- An `unmeasurable` entry: give each repeat more work, or declare a wider bound if the change allows it.
- `unchanged` (with `second_pass: true`) lists `{file, reason}`, one entry per file: what must come out the same after both passes. A file is a `*.jsonl` of `/stand/out` by its path there, such as `orders.jsonl`, or `probe.<name>.jsonl` of an `after-workload` probe.

## line

`stop-slop-stand run --json` prints one line on every exit code. It holds only the exceptions: what passed is counted in `quiet`, and every number stays in `report.json` for `stop-slop hot` and people.

- The line has `run`, `report`, `seal`, `attempts`, `comparisons`, `measures`, `behaviour`, `fault`, `secondPass`, `flags`, `needs`, `failure`, `quiet`, `unmeasurable` and `next`.
  - `run` is the id of the run for `inspect`, `null` when the run never started. `report` is the path of `report.json`, `null` when the run failed.
  - `attempts` counts the runs whose samples this one pools, itself included. `comparisons` counts the passed runs with `--before` since the last seal, this one included. Both come with a report; `comparisons` is `null` without `--before`.
  - `behaviour` is `null` without `--before` or when "before" failed.
  - `quiet` counts the measures and files with no exception. `unmeasurable` counts, among them, the measures without an `expect` entry graded `unmeasurable`: the noise could hide a shift of 5%.
  - `next` holds the commands to run next, ready to paste. A failure and a refusal always have one.
- `seal` has `sealedAt`, `maxTolerance` and `journal`.
  - The first `run --before` of a session seals the recipe, and the comparisons run on that copy. `sealedAt` is the time of the last seal or reseal, `maxTolerance` is `max_tolerance` of `stand.toml`. `seal` is `null` without `--before` or without a report.
- A journal entry has `sealedAt` and `digest`, `resealedAt`, `digest` and `reason`, or `cleanedAt`.
  - `journal` lists every seal, reseal and `clean` of the session, oldest first, and outlives `clean`.
- A measure has `name`, `unit`, `src` and `median`, then `before`, `previous` and `expect`.
  - `name` is `<subject>.<metric>`: the subject is `stand`, a service, a port `<service>:<port>` or a volume `volume:<name>`. `unit` is `ms`, `bytes`, `count` or `share`. `src` is `stand`: the stand measured it, the harness cannot.
  - `stand.hot-owned` is the own time of the profiled functions in the files that differ between "before" and the head, `stand.hot-collateral` that of the functions in the other files. They exist with `--before` and `x-stand.profile`. A fix that gives a changed file back its base content moves that file's time into `stand.hot-collateral`.
  - `median` is of the samples of every run of the same trees and recipe: running again adds samples instead of starting over. In `report.json`, `samples` lists this run's own values only, while `median` and the spread cover every pooled run.
- `before` and `previous` of a measure have `median` and `verdict`. `before` is `null` without `--before`; `previous` compares with the last passed run of other trees or recipe.
  - `verdict` is `improved` or `regressed` when a measure where less is better moved beyond its noise, `changed` when a measure with no better side (connections, bytes) moved, `within-noise`, or `no-baseline` when there is nothing to compare with.
- `expect` of a measure has `change`, `reason`, `grade`, `postHoc`, `stale` and `acceptances`.
  - `expect` is `null` unless "before" passed. `change` is the cost `x-stand.expect.cost` declared, or the bound of an acceptance that replaced it; a measure without an entry is `none`, with `reason` `null`.
  - `grade` is `met` when the shift stays within `change`; `exceeded` when it goes further the declared way; `surprise` when it goes the other way, shows under `none`, or does not show under `down`; `unmeasurable` when the noise is wider than `change` allows (5% for `none` and `down`): a shift of that size would not show. `grade` is `null` for `unknown`. `stand.hot-owned` rising from 0, the own time of new functions, is `unmeasurable` with `reason` `new-code`, quiet and counted in `unmeasurable`: `stand.wall` and `stand.hot-collateral` carry the check.
  - `postHoc` is `true` when an `accept` of this head replaced the declared cost. `stale` is `true` when the measure has acceptances, but only of another head: none of them holds now. `acceptances` counts them all.
  - A measure is in the line when its grade is not `met`, when `change` is `unknown`, or with `postHoc` or `stale`; an `unmeasurable` one without an entry stays quiet. Without `--before` it is there when `previous` moved. A `surprise` or an `exceeded` gets an `accept` command in `next` when its bound fits `maxTolerance`; an `unmeasurable` entry gets `guide expect`.
- A need has `measure` and `reason`.
  - `needs` lists the measures with `change` `unknown`: a person judges their cost.
- A behaviour file has `file`, `src`, `noiseKnown`, `diffs`, `sample` and `path`.
  - `src` is `workload` for a file the harness wrote into `/stand/out`, `probe` for a probe's output.
  - A field that changes between repeats of "before" is noise and never a diff. `noiseKnown` is `false` with a single repeat of "before": then nothing is known to be noise.
  - `diffs` counts the differences from "before", one per field of a record; `sample` holds the first 3, and the file at `path` all of them.
- A diff has `record` or `key`, `path`, `before` and `head`.
  - `record` numbers the records of the file from 1; a probe with `key` names the record by its key instead. `path` is the JSON Pointer of the field; a record on one side only has `path` `""` and `null` on the other side.
- `fault` has `target`, `mode`, `changed`, `unknown` and `from`.
  - `fault` is `null` without `x-stand.fault`. Two repeats per version run with `target` down or slowed, and the report compares what they ended with, not their times: `workload` (its exit or why it had none), `<service> state` (`running` or `exited`), `<target> attempts` (connections: `0`, `1`, `2-3`, `4-10` or `>10`) and `<target> responses` (the HTTP classes of its answers, `none` without one).
  - `unknown` lists the categories nothing compares on: the repeats of "before" disagreed, or a repeat ended before the fault went on. A rerun runs the fault repeats again then.
  - `from` is the run whose repeats made the result when this run took it from an earlier run of the same trees and recipe, instead of repeating them; `null` when this run made it.
- A category change has `category`, `before` and `head`.
  - `changed` has one entry per value the head showed and "before" never did.
- A second-pass file has `file`, `before`, `diffs`, `sample`, `path` and `from`.
  - The second pass runs the workload twice on one state, without a restore. `secondPass` lists the files of `x-stand.expect.unchanged` that changed on the head from the first pass to the second, `[]` when all held; it is `null` without `x-stand.second_pass` or when the head's second pass failed.
  - `before` is `changed` when the file changed on "before" too, so the head did not cause it; `null` without "before". In a diff of the file at `path`, `before` is the first pass and `head` the second. `from` is as in `fault`.
- `failure` has `phase`, `service`, `repeat`, `cause`, `logs` and `probes`.
  - `phase` is where it stopped: `setup`, `ready` or `workload` when the app failed (exit 1), another step on a refusal or a breakage. `cause` is `exit-<code>`, `setup-timeout`, `timeout`, `died`, `unhealthy` or `cancelled`, or the error of a refusal or a breakage.
  - `logs` is the folder with the logs of the failed setup or repeat; `probes` are the outputs of the `on-failure` probes.
- A flag has `kind` and the facts of its kind. No flag is a failure; each says what to know before trusting the numbers:
  - `high-noise` (`measure`): a version's samples spread more than 20% between repeats, so a smaller shift does not show. Give each repeat more work. On the head alone, for a measure graded `met` whose shift is far wider than the spread, it asks for nothing.
  - `first-repeat-outlier` (`measure`): the first repeat lies far from the others, as a warm-up does; the median already sets it aside.
  - `low-repeats` (`samples`): a version has fewer than 3 samples. Run again.
  - `no-profile` (`service`): a repeat of this profiled service wrote no profile; it needs Node 20 or later.
  - `unprofiled`: no service has `x-stand.profile`, so the report has no hot function and no coverage.
  - `lost-processes` (`service`, `count`): Node processes killed before they wrote their first profile chunk.
  - `unmapped-time` (`service`, `share`): this share of the service's TypeScript time found no source map; the hot functions miss it.
  - `memory-near-limit` (`service`, `peak`, `limit`): the service reached 90% of its memory limit.
  - `network-limit-hit` (`repeat`, `target`, `drops`): the stand's network dropped new connections to `target` after about 256 in 10 seconds to each of its replicas. It is not the app: give the workload keep-alive or a pool.
  - `missing-expose` (`name`, `attempts`): a service called `name`, which has no `expose`.
  - `before-failed` (`stage`): "before" failed at this stage, so nothing was compared with it.
  - `coverage-unknown` (`stage`, `service`): the report has no coverage, so `stop-slop hot` cannot list changed functions that never ran. The reason is one of: repeat failed at `stage`; `service` left no coverage (no profile in a measured repeat and no call in the coverage repeat, so it may have run unseen); no service in the coverage repeat (both `null`).
  - `previous-unreadable`: the previous report did not read; nothing is compared with it.
  - `recipe-changed`: the recipe changed since the previous run, so the comparison with it may be unfair.
  - `no-results` (`file`): with `--before`, `file` exists on one version only, or no `*.jsonl` exists at all (`file` is `null`).
  - `bad-result-line` (`file`, `line`): `line` is the first line of a workload file that is no JSON; every such line is left out.
  - `dead-probe` (`probe`, `reason`): the probe can never show a change: no record at all (`no-records`) or `ignore` takes every field (`all-ignored`).
  - `probe-failed` (`probe`, `side`, `repeat`, `code`): the probe exited with `code`, or hit its deadline when `code` is `null`; `repeat` is `null` after setup. An `on-failure` probe gives none: its output is in `failure.probes`.
  - `head-only-noise` (`file`, `path`): the field holds on "before" and changes between repeats of the head; it stays in the diff as a change.
  - `killed-at-stop` (`service`, `side`, `repeat`): the service ignored its stop signal for `stop_grace_period` and was killed.
  - `second-pass-failed` (`side`, `stage`): the second pass of that version failed at `stage`, so the workload did not run twice on one state. Its logs are in `pass2-<side>/` of the run.
  - `connections-grow` (`target`, `side`, `first`, `second`): the open connections of the long-running services to `target` grew from `first` after the first pass to `second` after the second: a pool or a client that never closes them. The workload's do not count: they close after it exits.
  - `unknown-expect` (`measure`): an `expect.cost` entry names a measure this run does not have, so nothing graded it.
  - `before-after-session` (`takenAt`, `startedAt`): "before" was taken after the session began, so it may hold part of the change. Take the `snapshot` before the first edit.
  - `measured-before-seal` (`run`): that run measured this head before the recipe was sealed, so the declared cost came after its numbers.
  - `wide-tolerance` (`maxTolerance`): `max_tolerance` in `stand.toml` is above 25%, so `accept` takes wide bounds.

## inspect

`stop-slop-stand inspect <run> <side> <service> -- <argv>` runs a command in a copy of the state a run ended with, to see what changed. It never gives a verdict.

- `<run>` is `run` of the line; only the kept run of the session works, the last one that kept snapshots. `<side>` is `head` or `before`. `<service>` is a service with a volume: only those keep an end-of-run snapshot.
- The copy runs alone, without its neighbours and without network, with the command of its service; a service that cannot start that way is exit 2 with the reason.
- `argv` runs under 60 seconds. The first line is `exit <code>` or `timeout`, then at most 40 lines and 64 KiB of its stdout, then `full output <path>`; its stderr is in `<path>.err`. The code of `argv` never becomes the exit code.
  - When `argv` left something other than a regular file at that path, such as a link, one line says so and nothing of it is printed.
  - Windows PowerShell 5.1 drops the double quotes inside an argument it passes on, so an `argv` such as `sh -c` with `"` in its script reaches the guest broken and may still exit 0. Keep `"` out of `argv`: single quotes, as in `-- psql -U postgres -At -c 'select count(*) from orders'`, or a script of the harness.
- A write lands in the copy only: the kept snapshot stays as it was.
- After a failed head (exit 1), `next` names the `inspect` of each volume service with the state of the failure. A probe (`guide probes`) is what turns a look into a measured fact.
