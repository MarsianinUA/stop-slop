# The stand

`stop-slop-stand` runs the app of the repository in microsandbox VMs from a Compose file: the version before a change and the version after it, a few repeats each. It measures both and prints one line of JSON with only what needs a look.

`run` prints nothing until its line, which takes 1-3 minutes, more with a bigger workload. Run it in the foreground with a 10-minute timeout: never in the background, and never poll it. If the tool stops waiting first, the run may still be going: wait until it ends or the harness says it ended. Do not start a second `run`: while the first holds the stand, it prints `waiting for the stand` and waits as long as a whole run may take, then exits 2. If the tool killed the run, it has no line and counts as failed. Its VMs may run on for as long as a whole run may take; the next `run` or `clean` drops them first.

The stand file `compose.yaml` and the harness beside it, such as the workload, are the recipe. They live in `<common>/info/stop-slop-stand/`, outside the repository, and every work tree of the repository shares them. `stop-slop-stand guide <topic>` explains one part of it: run the topic a step names before you do that step. A topic you have run needs no second run.

Two flows use these steps:

- **fix**, when [fix.md](fix.md) measures its fixes: [Before the first edit](#before-the-first-edit) before it edits a file, [Measure](#measure) after its tests, [Clean](#clean) last.
- **branch**, when the user asks what their branch costs: [The branch flow](#the-branch-flow).

## Exit codes

They hold for `run`; the other commands exit 0, 2, 3 or 130 the same way.

- `0`: the head passed. The line holds facts to read, not failures: step 6.
- `1`: the head failed, and nothing was measured. Read `failure`: `phase`, `service`, `cause`, the `logs` folder and the outputs of the `on-failure` probes. Run what `next` names (an `inspect` of the failed state, or `guide recipe`), fix the cause, and run again. The cause is the code when the change broke the app, the recipe when the app fails in it as it was.
- `2`: the stand refused its input and measured nothing. stderr says why and ends with `next:` lines: fix what it names in the recipe, or run what `next` names, and run again. What only the user can change, such as the disk, `host_share` in `stand.toml` or another run that holds the stand, goes to the user, and the task goes on without the measure.
  - A recipe changed after its seal (step 6) is such a refusal: put the file back, or seal again with `--reseal --reason "<why>"`. The line shows each reseal.
  - A missing snapshot after your first edit is the one `next` to pass over: a snapshot now takes your edits for "before". Compare with `--before <base.ref>` of `check` instead, and say in the report that this measured the whole branch.
- `3`: the stand itself broke. Show stderr and the line to the user and stop measuring: run it again only when the user asks, and change neither the recipe nor the code for it. The task goes on, and its report says the stand did not measure, and why.
- `130`: Ctrl+C stopped it. Go on as after `3`.

## Before the first edit

1. `stop-slop-stand snapshot`: it records the work tree, uncommitted files too, as "before" of this session. Take it before the task edits the first file: a later snapshot takes your edits for "before".
2. No `compose.yaml` in the recipe folder: write the recipe.
   - `stop-slop-stand init` writes a stand file to start from, with comments. Make it run this app (`guide recipe`): its images and limits, a prepare service that installs and builds, migrations, a stub for each outside API it calls. The project's own Compose file, if there is one, is a start.
   - Write the workload beside it, such as `load.mjs` (`guide recipe`, and "What the workload records" of `guide probes`): it calls the routes that reach the code of the branch, over keep-alive, and writes what the app answers into `/stand/out/*.jsonl`, one JSON record per line.
   - Add a probe for the state the branch writes, adapted from an example of `guide probes`, such as the rows of the tables its code writes. A branch that writes none still gets one probe of the state the app keeps.
   - Give each service that serves the workload `x-stand: { profile: node }`, not the setup services: then `hot` can name its hot functions and the changed code that never ran.
3. `stop-slop-stand run --dry-run`: it checks the recipe, the budget and the images without a VM, and drops nothing. Fix what it refuses until it exits 0.
4. A recipe of step 2: `stop-slop-stand run --json` once. No code has changed yet, so a failure here is the recipe's: fix the recipe until the run passes. Read its flags too: `high-noise` on a measure the change should move means the workload does too little to show that change, so give it more to do now, before the seal.

After the third failed `run --dry-run` or `run --json`, go on without the measure; the report says why. Done when `run --dry-run` exits 0, and a recipe of step 2 passed one `run --json` with no `probe-failed` or `dead-probe` in its `flags`.

## Measure

Measure once the code differs from the snapshot. With no edit, go to [Clean](#clean); the report says there was nothing to measure.

5. Declare the cost of the change in `x-stand.expect.cost` of the stand file, before the first `run --before` of the session, replacing a `cost` an earlier task left there (`guide expect`). Judge it from the diff by what the change intends, before any number of it: an entry `{measure, change, reason}` for each measure the change is meant to move, such as `{measure: stand.wall, change: down, reason: "one Set lookup instead of a scan per order"}`. A refactor intends no move: `cost: []`. Leave every other measure out: it is graded as `none` all the same, and stays quiet where its noise is too wide to grade, while an entry `none` there shows as `unmeasurable`. `unknown` hands a measure to the user. The measures are `stand.wall`, `stand.hot-owned`, `stand.hot-collateral`, `<service>.cpu`, `.memory`, `.error-lines` and `.shutdown`, `<service>:<port>.connections`, `.bytes` and `.http-5xx-share`, and `volume:<name>.bytes` (`guide line`).
6. `stop-slop-stand run --before session --json`, or `--before <base.ref>` in the branch flow. The first such run seals the recipe with the declared cost. Act on the exit code; on `0` read the line (`guide line`). It holds every declared measure with the numbers the report needs, and the exceptions, each of which gets an outcome; `report.json` holds more, such as a verdict per hot function, as context that needs none:
   - a measure graded `surprise` or `exceeded`. First rule out noise: a shift in a measure the diff cannot reach, or of a few clock ticks, such as `db.cpu` in steps of 15.6 ms on Windows, is no finding yet. Run again, so the samples pool, and judge the pooled line. Then find why, in the diff and with `inspect` (`guide inspect`); for a shift in time, run `stop-slop hot` on this line's `report` (step 7) now, before any fix: it shows where the own time went and the findings inside, and a fix that gives a file back its base content takes that file out of the branch. A shift the change does not intend, however wide, is a regression: fix the code. A shift the change intends: run the `accept` command of `next`, with that reason; with no `accept` there, the shift is wider than `max_tolerance`, and the user judges it.
   - a measure graded `unmeasurable`: the noise hides the declared change. Give each repeat more work, which narrows the spread (more `repeat` does not), or declare a bound the change allows (`guide expect`). The `reason` `new-code` needs neither.
   - `needs`: measures the user judges; they go to the report.
   - a file in `behaviour`, a category in `fault.changed`, a file in `secondPass`: the app now answers, writes or recovers otherwise. A change the diff intends goes to the report with why; any other is a regression: fix the code.
   - a flag: `guide line` says what it means and what to do, such as `low-repeats` (run again) or `dead-probe` (fix the probe). In the fix flow, `recipe-changed` on the first runs with `--before` says only that `expect` came after the run it compares with; after a reseal, so do `recipe-changed` and `measured-before-seal`.

   After an edit of the code, check it before the next run: in the fix flow steps 7-9 of fix.md again; in the branch flow `stop-slop check --json`, so the edit adds no finding, then steps 8 and 9 of fix.md. A test the edit breaks takes the edit back. Then run again. Done when every exception of the last line has its outcome: fixed, accepted with a reason, or in the report. After the third run with `--before`, stop and report what is still open.
7. `stop-slop hot <report> --json`, with `report` of the last line that passed, and the `--base` of `check` when it needed one. After a fix of a regression, its `hot` of step 6 goes to the report too. For the files of the branch it lists:
   - `hot`: the hot functions, the hottest first by own time (`selfMs`), each with the `check` findings on its lines (`findings`, their rules in `rules`). A finding in a hot function costs the most: it is the first suspect of a regression in time.
   - `unexercised`: the functions on changed lines that the workload never entered, so the stand measured nothing of them.
   - `coverage`: `not-collected` when no service has `x-stand.profile`, `unknown` when the line flagged `coverage-unknown`. Then `unexercised` is `null`, and the report says the stand could not tell.
   - `stale: true` on a row: its file changed after the run, so its lines may have moved; run the stand again before you trust it.

   It exits 1 when it lists anything, an `unexercised` function alone too. On 2 or 3 show its stderr and go on without `hot`. Done when each `hot` row with findings is fixed (in the branch flow only as the regression of step 6) or in the report, and each `unexercised` function is named in the report.

## Clean

8. `stop-slop-stand clean`, once, as the last command of the task, also when the stand did not measure. It drops the VMs, snapshots, images, seals, setup caches and run folders of every work tree that uses the stand, so leave it out only when the user says another task uses the stand now. Before it, copy into the report the numbers it needs, since the `report.json` files go too, and take this task's `cost` out of the stand file: the file is shared, and the next task declares its own. A workload the task enlarged stays as it is; its reason is in the journal of the seal. It prints what it removed, such as `cleaned 1 session, 3 snapshots`. Done when it exits 0; on 2 or 3 its stderr goes to the report.

## Report

The stand's part of the report:

- what "before" was (the snapshot of the session, or `base.ref`), and `comparisons`;
- the declared cost, and each measure's grade against it;
- each exception with its outcome: what you fixed and in which file, what you accepted and why, what is open; a fix that undoes a hunk of the user's own says so;
- `needs`, for the user to judge;
- from `hot`: the hot functions with findings inside, and the changed functions that never ran;
- or that the stand did not measure, and why.

## The branch flow

The user asks what their branch costs, or to measure it before a PR. The branch is theirs: edit its code only to remove a regression the stand shows, and leave the commit to them.

1. `git fetch`, then `stop-slop check --json`: its `base.ref` is "before", and its findings are what `hot` places in hot functions.
2. Step 2 when there is no stand file yet, then step 3 as `run --before <base.ref> --dry-run`: it checks the seal too and seals nothing. Step 4 would run the branch, not code known to work, so the first run is step 6.
3. Steps 5-7, with `run --before <base.ref> --json`. The cost to declare is what the branch's diff against `base.ref` is meant to do. A head that fails (exit 1) while "before" passed is a regression of the branch; with `before-failed` among the flags, the recipe fails both versions: fix the recipe.
4. Step 8, then the report, with every edit of the branch by file, so the user can review it before the commit.
