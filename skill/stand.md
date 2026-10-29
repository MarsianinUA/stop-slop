# The stand

`stop-slop-stand` runs the app of the repository in microsandbox VMs from a Compose file: the version before a change and the version after it, a few repeats each. It measures both and prints one line of JSON with only what needs a look.

The stand file `compose.yaml` and the harness beside it, such as the workload, are the recipe. They live in `<common>/info/stop-slop-stand/`, outside the repository, and every work tree of the repository shares them. `stop-slop-stand guide <topic>` explains one part of it; each step names the topic it needs.

Two flows use these steps:

- **fix**, when [fix.md](fix.md) measures its fixes: [Before the first edit](#before-the-first-edit) before it edits a file, [Measure](#measure) after its tests, [Clean](#clean) last.
- **branch**, when the user asks what their branch costs: [The branch flow](#the-branch-flow).

## Exit codes

They hold for `run`; the other commands exit 0, 2, 3 or 130 the same way.

- `0`: the head passed. The line holds facts to read, not failures: step 6.
- `1`: the head failed, and nothing was measured. Read `failure`: `phase`, `service`, `cause`, the `logs` folder and the outputs of the `on-failure` probes. Run what `next` names (an `inspect` of the failed state, or `guide recipe`), fix the cause, and run again. The cause is the code when the change broke the app, the recipe when the app fails in it as it was.
- `2`: the stand refused its input and measured nothing. stderr says why and ends with `next:` lines: fix what it names in the recipe, or run what `next` names, and run again. What only the user can change, such as the disk, `host_share` in `stand.toml` or another run that holds the stand, goes to the user, and the task goes on without the measure. A missing snapshot after your first edit is the one `next` to pass over: a snapshot now takes your edits for "before". Compare with `--before <base.ref>` of `check` instead, and say in the report that this measured the whole branch.
- `3`: the stand itself broke. Show stderr and the line to the user and stop measuring: run it again only when the user asks, and change neither the recipe nor the code for it. The task goes on, and its report says the stand did not measure, and why.
- `130`: Ctrl+C stopped it. Go on as after `3`.

A change of the recipe after its seal (step 6) runs with `--reseal --reason "<why>"`; the line shows each one.

## Before the first edit

1. `stop-slop-stand snapshot`: it records the work tree, uncommitted files too, as "before" of this session. Take it before the task edits the first file: a later snapshot takes your edits for "before".
2. No `compose.yaml` in the recipe folder: write the recipe.
   - `stop-slop-stand init` writes a stand file to start from, with comments. Make it run this app (`guide recipe`): its images and limits, a prepare service that installs and builds, migrations, a stub for each outside API it calls. The project's own Compose file, if there is one, is a start.
   - Write the workload beside it, such as `load.mjs`: it calls the routes that reach the code of the branch, over keep-alive, and writes what the app answers into `/stand/out/*.jsonl`, one JSON record per line.
   - Add a probe for the state the branch writes, adapted from an example of `guide probes`, such as the rows of the tables its code writes.
   - Give each service that runs the app's Node code `x-stand: { profile: node }`: then `hot` can name its hot functions and the changed code that never ran.
3. `stop-slop-stand run --dry-run`: it checks the recipe, the budget and the images without a VM, and drops nothing. Fix what it refuses until it exits 0.
4. A recipe of step 2: `stop-slop-stand run --json` once. No code has changed yet, so a failure here is the recipe's: fix the recipe until the run passes.

Done when `run --dry-run` exits 0, and a recipe of step 2 passed one `run --json`.

## Measure

Measure once the code differs from the snapshot: with no edit, `run --before session` exits 2, since there is nothing to compare.

5. Declare the cost of the change in `x-stand.expect.cost` of the stand file, before the first `run --before` of the session (`guide expect`). Judge it from the diff, before any number of it: an entry `{measure, change, reason}` for each measure the change should move, such as `{measure: stand.wall, change: down, reason: "one Set lookup instead of a scan per order"}`. Every other measure is `none`; `unknown` hands one to the user.
6. `stop-slop-stand run --before session --json`, or `--before <base.ref>` in the branch flow. The first such run seals the recipe with the declared cost. Act on the exit code; on `0` read the line (`guide line`). It holds only exceptions, and each one gets an outcome:
   - a measure graded `surprise` or `exceeded`: find why, in the diff and with `inspect` (`guide inspect`); for a shift in time, `stop-slop hot` on this line's `report` (step 7) shows where the own time went and the findings inside. A shift the change explains and intends: run the `accept` command of `next`, with that reason; with no `accept` there, the shift is wider than `max_tolerance`, and it goes to the user. Any other shift is a regression: fix the code.
   - a measure graded `unmeasurable`: the noise hides the declared change. Give the workload more to do or raise `repeat`, or declare a bound the change allows (`guide expect`).
   - `needs`: measures the user judges; they go to the report.
   - a file in `behaviour`, a category in `fault.changed`, a file in `secondPass`: the app now answers, writes or recovers otherwise. A change the diff intends goes to the report with why; any other is a regression: fix the code.
   - a flag: `guide line` says what it means and what to do, such as `low-repeats` (run again: runs of the same trees and recipe pool their samples) or `dead-probe` (fix the probe).

   After an edit of the code, check it before the next run: in the fix flow steps 7-9 of fix.md again; in the branch flow `stop-slop check --json`, so the edit adds no finding, then steps 8 and 9 of fix.md. A test the edit breaks takes the edit back. Then run again. Done when every exception of the last line has its outcome: fixed, accepted with a reason, or in the report. After the third run with `--before`, stop and report what is still open.
7. `stop-slop hot <report> --json`, with `report` of the last line that passed. For the files of the branch it lists:
   - `hot`: the hot functions, the hottest first by own time (`selfMs`), each with the `check` findings on its lines (`findings`). A finding in a hot function costs the most: it is the first suspect of a regression in time.
   - `unexercised`: the functions on changed lines that the workload never entered, so the stand measured nothing of them. Name each in the report.
   - `coverage`: `not-collected` when no service has `x-stand.profile`, `unknown` when the line flagged `coverage-unknown`. Then `unexercised` is `null`, and the report says the stand could not tell.
   - `stale: true` on a row: its file changed after the run, so its lines may have moved; run the stand again before you trust it.

   Exit 1 means it listed something; done when each row is fixed or in the report. On 2 or 3 show its stderr and go on without `hot`.

## Clean

8. `stop-slop-stand clean`, once, as the last command of the task, also when the stand did not measure. It drops the VMs, snapshots, images, seals and setup caches of every work tree that uses the stand, so leave it out only when the user says another task uses the stand now. Done when it exits 0; on 2 or 3 its stderr goes to the report.

## Report

The stand's part of the report:

- what "before" was (the snapshot of the session, or `base.ref`), and `comparisons`;
- the declared cost, and each measure's grade against it;
- each exception with its outcome: what you fixed and in which file, what you accepted and why, what is open;
- `needs`, for the user to judge;
- from `hot`: the hot functions with findings inside, and the changed functions that never ran;
- or that the stand did not measure, and why.

## The branch flow

The user asks what their branch costs, or to measure it before a PR. The branch is theirs: edit its code only to remove a regression the stand shows, and leave the commit to them.

1. `git fetch`, then `stop-slop check --json`: its `base.ref` is "before", and its findings are what `hot` places in hot functions.
2. Steps 2 and 3, when there is no stand file yet. Step 4 would run the branch, not code known to work, so the first run is step 6.
3. Steps 5-7, with `run --before <base.ref> --json`. The cost to declare is that of the branch's diff against `base.ref`. Each exception gets its outcome as in step 6: a fix of the branch's code, an `accept` with a reason, or the report. A head that fails (exit 1) while "before" passed is a regression of the branch; with `before-failed` among the flags, the recipe fails both versions: fix the recipe.
4. Step 8, then the report, with every edit of the branch by file, so the user can review it before the commit.
