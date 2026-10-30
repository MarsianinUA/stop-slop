---
name: stop-slop
description: stop-slop convention linter for TypeScript, with a stand that measures the app in VMs. Use when the user mentions stop-slop and wants to fix the findings on their own branch, measure their branch on the stand, draft review comments on a colleague's PR, or send an approved draft.
---

stop-slop checks the TypeScript files a branch changes against personal conventions, and its stand measures what the branch costs the running app. The programs find and measure; you judge, fix and write. Every run takes one branch:

- **Fix** the user's own branch: read [fix.md](fix.md) and follow it. It also says when to measure the fixes on the stand.
- **Measure** the user's own branch on the stand, when they ask what it costs: read [stand.md](stand.md) and follow its branch flow. The stand runs only the user's own code: a colleague's PR is reviewed, never run.
- **Review** a colleague's PR (the user gives its number or link): read [review.md](review.md). The run ends on a draft the user edits.
- **Send** a draft the user approved ("send", "отправляй"): read [send.md](send.md).

## The program

Run `stop-slop` inside the repository. If the command is missing, tell the user to install it with `cargo install --path crates/cli` from the stop-slop repository, and stop. stop-slop keeps its files in `<common>/info/stop-slop/`, where `<common>` is `git rev-parse --git-common-dir`.

- Exit codes: `0` nothing found, `1` findings, `2` the program refused its input (a bad `config.toml` names the key), `3` the program itself broke, `130` Ctrl+C stopped it. On 2 and 3, show its stderr to the user and stop: it ends with a `next:` line for each command that may help. `--json` then prints `{"failure": {"phase", "cause"}, "next": [..]}` instead of its usual line.
  - The one 2 you answer yourself is `no base branch ...; pass --base`, as on a branch that sits on the tip of `main`: give `--base` the branch the user's branch started from, such as `main`, to `check`, `ignore` and `hot` alike, and name it in the report.
- `--json` prints one line. Leave `--pretty` off: it indents the same JSON by 2 spaces for a person, and the indents cost you tokens.
- `stop-slop check --json` prints `pr` (`null` without `--pr`), `base`, `files` (the branch files it checked: commits, uncommitted edits and untracked files), `skipped`, `findings` and `fixed` (`rule`, `file`, `count` of the fixes `--fix` applied).
  - `pr` has `number`, `head` (the full sha), `base` (the branch name, without `origin/`) and `title`.
  - `base` has `ref`, `sha` and `reason`: `flag` (given with `--base`), `pr` (the base of the PR: of `--pr`, or of the branch), `ancestor` (the nearest ancestor branch) or `default` (`origin/HEAD`).
- A finding has `rule`, `kind`, `file`, `line`, `column`, `endLine`, `endColumn`, `message`, `inHunk`, `fixable`, `fingerprint`.
  - `kind`: `error` means the rule is sure; `candidate` is a heuristic you decide with the rule's `agent` hint; `parse-error` means the file did not parse, so no rule ran on it.
  - `inHunk: true` puts the finding on a line the branch changed; `false` is old code in a touched file.
  - `message` says why the code is a problem and how to fix it.
  - `fixable: true` means `--fix` applies the rule's fix at this place. Every other finding you fix by hand, even when the rule has `fix: true`: there the program judged its fix unsafe.
- `stop-slop rules --json` lists the rules: `id`, `group`, `description`, `kind` (`error`, `candidate` or `agent`), `fix`, `files` (`code`, `tests` or `all`), `source`, `message`, `agent` (how to judge, fix or check the rule; `null` when the `message` says enough) and `active` (on in this repository). It is the only list of rules: read it once per run.
  - An `agent` rule has no code in the program. You check it yourself, as its `agent` text says, in the branch files its `files` covers.
  - Test files end in `.spec`, `.test` or `.e2e-spec` before the extension, or sit under `__tests__/`. Every other file is code.
  - `enabled` in `<common>/info/stop-slop/config.toml` turns opt-in groups on for this repository, such as `performance` and `branching`; a rule `disabled` names stays off even then. `disabled` is the one key you change, as [Project conventions win](#project-conventions-win) says.

## The stand

`stop-slop-stand` runs the app in microsandbox VMs; only [stand.md](stand.md) calls it. If the command is missing, tell the user to install it with `cargo install --path stand/crates/cli --locked` from the stop-slop repository; it needs a hypervisor (WHP on Windows, KVM on Linux, Apple silicon on a Mac). Then a fix goes on without the stand, and a measure stops.

- Its exit codes are `0` passed, `1` the head failed, `2` refused, `3` broke and `130` Ctrl+C, as stop-slop's, but they ask other steps: [stand.md](stand.md#exit-codes).
- `stop-slop-stand --help` lists its commands and flags, and `stop-slop-stand guide <topic>` explains one part of the stand: `recipe`, `probes`, `expect`, `line` or `inspect`. The topics are [references/guide.md](references/guide.md) and [references/probes.md](references/probes.md).
- `stop-slop hot <report.json> --json` lays the report of a stand run over the branch: its hot functions with the findings inside, and its changed functions that never ran ([stand.md](stand.md), step 7).

## Project conventions win

Read the project's conventions before you judge findings: every `CONVENTIONS.md`, `CLAUDE.md` and `CONTRIBUTING*` in the repository. When a convention directly contradicts a rule (what the rule flags is the convention's subject, and the convention allows or requires it; a name in its example does not count), the project wins: add the rule id to `disabled` in `<common>/info/stop-slop/config.toml`. Create the file if needed, keep what it already holds, and name the rule and the convention in the report.

```toml
disabled = ["no-boolean-param"]
```
