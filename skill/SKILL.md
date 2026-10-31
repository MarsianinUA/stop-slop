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
- `stop-slop check --json` prints `pr` (`null` without `--pr`), `base`, `files` (the branch files it checked: commits, uncommitted edits and untracked files), `skipped`, `findings`, `fixed` (`rule`, `file`, `count` of the fixes `--fix` applied) and `rules` (the entries of `stop-slop rules --json` for the rules of its findings and the active `agent` rules).
  - `pr` has `number`, `head` (the full sha), `base` (the branch name, without `origin/`) and `title`.
  - `base` has `ref`, `sha` and `reason`: `flag` (given with `--base`), `pr` (the base of the PR: of `--pr`, or of the branch), `ancestor` (the nearest ancestor branch) or `default` (`origin/HEAD`).
- A finding has `rule`, `kind`, `file`, `line`, `column`, `endLine`, `endColumn`, `function`, `inHunk`, `touched`, `fixable`, `fingerprint` and `detail` (the values of its rule's `message`, only when it has some).
  - `kind`: `error` means the rule is sure; `candidate` is a heuristic you decide with the rule's `agent` hint; `parse-error` means the file did not parse, so no rule ran on it. It has no entry in `rules`: its `detail.error` says why the file did not parse.
  - `function` names the code around the finding: the innermost function (for a parameter, the one that declares it), else the class field, else the top-level statement. A method reads `Class.method`, an arrow its variable, a callback `(anonymous)`, and code outside any declaration, such as an import, `(top level)`. A finding in a comment belongs to the code the comment is about. It is `null` for a finding about the file itself: `file-name-pattern`, `kebab-case-dirs` and `parse-error`.
  - `touched: true` means the diff changes or removes a line of that code, not only comments or blank lines; for a finding about the file itself, that the branch adds or renames the file.
  - `inHunk: true` puts the finding on a line the branch changed; `false` is old code in a file the branch changed.
  - `detail` fills its rule's `message`: each `{key}` there stands for `detail.<key>`, such as `{example}`, the file name that `file-name-pattern` suggests.
  - `fixable: true` means `--fix` applies the rule's fix at this place. Every other finding you fix by hand, even when the rule has `fix: true`: there the program judged its fix unsafe.
- A rule in `rules` has `id`, `group`, `description`, `kind` (`error`, `candidate` or `agent`), `fix`, `files` (`code`, `tests` or `all`), `source`, `message` (why the code is a problem and how to fix it), `agent` (how to judge, fix or check the rule; `null` when the `message` says enough) and `active` (on in this repository).
  - An `agent` rule has no code in the program. You check it yourself, as its `agent` text says, in the branch files its `files` covers.
  - Test files end in `.spec`, `.test` or `.e2e-spec` before the extension, or sit under `__tests__/`. Every other file is code.
  - `enabled` in `<common>/info/stop-slop/config.toml` turns opt-in groups on for this repository, such as `performance` and `branching`; a rule `disabled` names stays off even then. `disabled` is the one key you change, as [Project conventions win](#project-conventions-win) says.

## The stand

`stop-slop-stand` runs the app in microsandbox VMs; only [stand.md](stand.md) calls it. If the command is missing, tell the user to install it with `cargo install --path stand/crates/cli --locked` from the stop-slop repository; it needs a hypervisor (WHP on Windows, KVM on Linux, Apple silicon on a Mac). Then a fix goes on without the stand, and a measure stops.

- Its exit codes ask other steps than stop-slop's: [stand.md](stand.md#exit-codes).
- `stop-slop-stand --help` lists its commands and flags, and `stop-slop-stand guide <topic>` explains one part of the stand: `recipe`, `probes`, `expect`, `line` or `inspect`. The files in `references/` hold these topics: read them only through `guide`, a topic when a step names it, and never a whole file.
- `stop-slop hot <report.json> --json` lays the report of a stand run over the branch: its hot functions with the findings inside, and its changed functions that never ran ([stand.md](stand.md), step 7).

## Project conventions win

Read the project's conventions before you judge findings: every `CONVENTIONS.md`, `CLAUDE.md` and `CONTRIBUTING*` in the repository. When a convention directly contradicts a rule (what the rule flags is the convention's subject, and the convention allows or requires it; a name in its example does not count), the project wins: add the rule id to `disabled` in `<common>/info/stop-slop/config.toml`. Create the file if needed, keep what it already holds, and name the rule and the convention in the report.

```toml
disabled = ["no-boolean-param"]
```
