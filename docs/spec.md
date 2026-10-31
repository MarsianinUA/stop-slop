# stop-slop specification (v1)

## Purpose

stop-slop is a personal convention linter for TypeScript. It checks the files of the current branch, then either fixes what it finds or drafts review comments for a colleague's pull request.

Why a tool of its own and not rules in the project's linter:

- A team will not change its lint config for one person's conventions.
- Only the files of the branch matter. stop-slop flags old debt only in files the branch already touches, by the boy-scout rule.
- Lint scripts in TypeScript or `.mjs` were too slow. They ran on one thread, one file after another. stop-slop is a native program and checks files in parallel.

## Parts

1. **The program `stop-slop`**, in Rust, on the [oxc](https://github.com/oxc-project/oxc) parser, the same one oxlint uses.
   - It finds violations and applies the fixes that keep the code's behavior.
   - It reads syntax and scopes only. It has no TypeScript type checker, which keeps it fast and limits what a rule can know.
   - It makes no network calls of its own. It runs `git`, and `gh` to look up pull requests.
2. **The skill** in `skill/`, for a coding agent. The agent runs the program, judges the findings, fixes what the program cannot, and writes review comments.
3. **The agent hook.** It checks a file right after the agent edits it, in repositories where you ran `stop-slop enable`.
4. **The stand `stop-slop-stand`**, a second program in Rust, in its own workspace `stand/`. It runs my app from a Compose file in [microsandbox](https://github.com/superradcompany/microsandbox) VMs, before and after my change, measures both, and prints what an agent needs to look at as one line of JSON. See [The stand](#the-stand). The two programs share no code but the crates of `shared/`.

## Modes

The skill has four branches, and each run takes one: fix the user's branch, measure it with the stand, review a colleague's PR, or send a review draft the user approved. The user names stop-slop in the request: "fix my branch with stop-slop", "measure my branch on the stop-slop stand", "review PR 123 with stop-slop", then "send it".

### Fix your branch

The agent fixes everything at once, with no approval step. The run ends when `check` is clean, with no `error` left and every `candidate` either fixed or ignored with a reason. A `performance` or `branching` fix the project's tests reject is the exception: it is reverted, and its finding goes to the report.

1. `git fetch`, so the base is fresh. With no remote, or when the fetch fails, the run goes on and the report says so.
2. `stop-slop check --json`. The agent keeps this first list for the report, outside the repository. Its `inHunk` tells the user's own lines from cleanup in old code. Its `rules` hold the rules of the findings and the active `agent` rules.
3. The agent reads the project's conventions, see [Project conventions win](#project-conventions-win).
4. `stop-slop check --fix --json`. The program applies its safe fixes and prints what is left.
5. The agent goes through the findings:
   - it fixes an `error` as its rule's `message` says and the rule's `agent` text advises;
   - it judges a `candidate`, which is a heuristic, by the rule's `agent` hint. It fixes a real problem and ignores a false one at the end, with a reason;
   - it fixes a `parse-error` when the branch broke the syntax, and reports it otherwise.

   When a fix needs something the repository lacks, such as a logger or a config service, the agent makes the smallest change that meets the rule and says so. Moving or renaming a file or an exported name reaches other files. The agent updates every import and use, and lists those files in the report.
6. The agent checks each `agent` rule with `active: true` itself, in the branch files its `files` covers.
7. `check --fix --json` again. While an `error` or an undecided `candidate` remains, the agent goes back to step 5. Then it puts each candidate it judged fine into the exception list with `stop-slop ignore <fingerprint> --reason "..."`. Ignoring comes last, because editing the flagged code changes its fingerprint.
8. If the project uses `prettier`, `biome` or `dprint`, the agent runs it on the changed files. Then `check` runs once more and stays clean.
9. The agent runs the `typecheck` and `lint` scripts of `package.json`, if they exist. After a fix of a `performance` or `branching` finding, it runs the `test` script too. A fix the tests reject is reverted, and its finding stays open.
10. The agent leaves the commit to the user. The report gives:
    - the base and why the program picked it;
    - the fixes in the user's own lines, by rule;
    - the cleanup in old code, by file and line, so the user can commit it apart;
    - the files that renames and moves touched;
    - every ignored finding with its reason, and every rule a convention turned off;
    - the formatter, `typecheck`, `lint` and `test` results, and each `performance` or `branching` fix the tests rejected.

The fix also measures itself on [the stand](#the-stand) when the user asks for it, or when the repository has a stand file: then the user set the stand up for it.

- Before the first edit, after step 3, the agent takes the `snapshot` of the work tree. Without a stand file it writes one from `init`, with a workload and a probe, checks it with `run --dry-run` and runs it once: no code has changed yet, so a failure is the recipe's.
- After the tests, it declares the cost of its fixes in `x-stand.expect`, runs `stop-slop-stand run --before session --json`, and gives every exception of the line an outcome: a fix of the code, an `accept` with a reason, or a line in the report. Then `stop-slop hot` on the report names the hot functions with findings inside and the changed functions that never ran.
- `stop-slop-stand clean` is the last command of the task.

The report then also gives the declared cost and each grade, each exception with its outcome, the measures the user must judge, what `hot` found, or that the stand did not measure, and why.

### Measure your branch

The agent measures what the branch costs the running app. It edits the branch's code only to remove a regression the stand shows, and leaves the commit to the user. The stand runs only my own branch, never a colleague's PR.

1. `git fetch`, then `stop-slop check --json`, for `base.ref` and the findings.
2. Without a stand file, the agent writes one, as in the fix.
3. It declares the cost of the branch's diff against `base.ref` in `x-stand.expect`, then runs `stop-slop-stand run --before <base.ref> --json`.
4. It gives each exception of the line an outcome, as in the fix: an edit of the code that removes a regression, an `accept` with a reason for a shift the diff intends, or a line in the report. `inspect` and `stop-slop hot` show where to look. After an edit, `check` adds no finding, the formatter, `typecheck`, `lint` and `test` run as in the fix, and the stand runs again, three times with `--before` at most.
5. `stop-slop hot <report> --json` on the last report names the hot functions of the branch with findings inside, and the changed functions that never ran.
6. `stop-slop-stand clean`, then the report: the base, the declared cost and each grade, each exception with its outcome, every edit by file, what the user must judge, and what `hot` found.

### Review a colleague's PR

The working tree stays as it is. The run ends on a draft that the user edits.

1. `stop-slop check --pr <n> --json`. The program asks `gh pr view <n>` for the head, the base and the title of the PR, fetches the head into `refs/stop-slop/pr-<n>` and the base into `origin/<base>` without a checkout, and reads the files straight from git. When the fetched head is not the one `gh` named, the PR moved during the fetch: the program exits with 2, and the agent runs it again.
2. The agent takes the rules from the `rules` of `check`, and reads the project's conventions as they are at the PR head. `active` there comes from the `package.json` files of the PR's files at its head.
3. The agent drops:
   - false positives: a candidate its hint clears, or an error the code proves wrong;
   - what the project's conventions allow;
   - what is not the author's: an old finding with `inHunk: false` and `touched: false`, in code the PR left alone, unless the PR made it worse. A finding about the file itself, such as its name, is the author's only when the PR adds or renames the file, which is what its `touched` says.

   When most of the codebase follows a habit a rule flags and no convention mentions it, the agent keeps the finding and points it out to the user.
4. The agent checks each active `agent` rule in what the PR adds or changes.
5. The agent writes a comment for every finding left, except a `parse-error`, which goes to the user. Findings of one rule in one function share a comment.
6. The agent writes the draft to `<common>/info/stop-slop/reviews/pr-<n>.md`, see [Where the files live](#where-the-files-live). Its header holds the `number`, `title` and `head` of `pr` from the output of `check`. The user deletes the blocks they disagree with, edits the text, and tells the agent to send.

A review comment:

- is in English, two or three short sentences in plain words, with a code sample when it helps;
- is gentle and does not expect the author to know the whole codebase;
- says what to change and why;
- on old code, says the line is older than the PR and asks for the change while the function is open anyway;
- links the section of the project's conventions at the PR head, when they have the rule;
- goes through `unslop`, my writing skill that strips AI-sounding phrasing. The skill is not in this repository. Without it, the comment follows the points above.

A draft looks like this. Deleting a block drops its comment.

```md
# PR 123: Cache market prices
Head: 9f2c4e1d0b7a6c5e4f3a2b1c0d9e8f7a6b5c4d3e

## 1. src/modules/markets/markets.service.ts:42
Rule: no-process-env (CONVENTIONS.md → Config Access)

Could we read this through ConfigService instead of process.env? Runtime code gets its config from there, and process.env stays in `*.config.ts` files.

<!-- stop-slop:no-process-env:3f9a1c -->

## 2. src/modules/markets/markets.mapper.ts:8 (outside the diff)
Rule: no-boolean-param

...
```

- The text under `Rule:` is the comment exactly as the agent posts it.
- `(outside the diff)` marks a finding with `inHunk: false`.
- The last lines of a block are its markers, one per finding: `<!-- stop-slop:<fingerprint> -->`, or `<!-- stop-slop:<rule> -->` for an `agent` rule. They keep a second run from posting the same comment again.

### Send the review

The agent sends only after the user says so.

- All comments go out as one review with `event=COMMENT`.
- GitHub takes inline review comments only on lines inside the PR's diff. The agent puts a block inline when its line is an added line, and the rest into the review body, each under its `path:line` and with its markers. The diff decides, not the heading.
- The review targets the head the check ran on. When the PR has moved since then, the agent says so, offers a new review, and sends nothing.
- Before sending, the agent reads the comments and review bodies already on the PR and skips each block whose markers are there. A second run posts no duplicates.
- The agent writes the review next to the draft as `pr-<n>.json` and posts it with `gh api`.

### Project conventions win

Before judging findings, the agent reads every `CONVENTIONS.md`, `CLAUDE.md` and `CONTRIBUTING*` in the repository. When a convention directly contradicts a rule, the project wins. The agent adds the rule to `disabled` in `config.toml` and names the rule and the convention in its report.

## The hook

Register the hook once, globally, in `~/.claude/settings.json`:

```json
{
  "hooks": {
    "PostToolUse": [
      {
        "matcher": "Edit|Write|MultiEdit",
        "hooks": [{ "type": "command", "command": "stop-slop hook" }]
      }
    ]
  }
}
```

`stop-slop hook`:

1. Reads the hook's JSON from stdin and takes `tool_input.file_path`.
2. Exits with 0 when the file is not `.ts`, `.tsx`, `.mts` or `.cts`, or is a declaration file.
3. Exits with 0 when the file's repository has no `enabled` marker. A file outside a repository has no marker to read, so the hook exits with 0. So does a file in a folder git refuses, like one inside `.git/` or one with `dubious ownership`.
4. Checks the whole file with the `error` rules, without fixes. Candidates would get in the way in the middle of the work; the final `check` deals with them. A file that does not parse counts as an error too.
5. When there are errors, prints them to stderr and exits with 2. The agent gets a hook's stderr only on exit code 2, and then fixes the errors. Over 20 errors, the hook prints 19 and a line with the count of the rest.

When the program itself fails, it exits with 2 as well, so the agent sees the error text.

`stop-slop enable` creates the marker. `stop-slop disable` removes only the marker; `ignore.json` and `config.toml` stay. `enable` affects only the hook. `check` works in any repository.

The hook does not go into a project's `.claude/` folder. Teams often commit that folder, and stop-slop is a personal tool.

## Which files get checked

Only the files of the current branch. Git does not record the branch a commit came from, so the program first looks for the parent branch. Stacks of ten PRs, each on top of the one before, are normal, so `main` or `dev` will not do.

The base, in this order:

1. `--base <ref>`, when given.
2. The base of the branch's pull request, from GitHub through `gh pr view <branch> --json baseRefName`. In a stack, the base of PR 5 is the branch of PR 4. The program asks only when `origin/<branch>` exists, because a branch nobody pushed has no PR, and `gh` costs 0.3 to 1.5 seconds of network. It takes `origin/<base>` over a local branch of that name, which may be old. Without `gh` or without network, the program skips this step.
3. The nearest ancestor branch among the local branches and `origin/*`. That is the branch whose common commit with HEAD is closest to HEAD. A branch that contains HEAD does not count, because it could be the current branch, a descendant further up the stack, or the parent before the first commit. `origin/<current>` does not count either. On a tie, the branch with fewer commits of its own wins, then the first in ref order.
4. The default branch, `origin/HEAD`.

When none of these gives a base, `check` exits with 2 and asks for `--base`. That happens, for example, on a new branch with no commits of its own in a repository without `origin`.

The files:

- Everything changed since the common commit with the base: the branch's commits, uncommitted edits and untracked files. One `git diff -U0 --merge-base <base>` gives the files and their changed lines; `git ls-files --others --exclude-standard` adds the untracked ones.
- Only `.ts`, `.tsx`, `.mts` and `.cts`. The program leaves out deleted files and declaration files: `.d.ts`, `.d.mts`, `.d.cts`.
- **Squash protection.** When the parent PR was squash-merged into the base, its files show up in the diff too. A file whose content equals the file at the tip of the base has no change of the branch's own, so the program leaves it out and lists it in `skipped` as `already-in-base`.
- The output always names the base and why the program picked it, so a wrong base shows at once.

Each finding gets `inHunk`, which says whether the branch changed its first line, and `touched`, which says whether the branch changed the code around it, see [JSON output of `check`](#json-output-of-check).

## The program

### Commands

| Command | What it does |
|---|---|
| `stop-slop check [flags]` | Checks the files of the branch. |
| `stop-slop hook` | The hook, see [The hook](#the-hook). |
| `stop-slop ignore <fingerprint> --reason "..." [--base <ref>]` | Adds an exception. It finds the file itself by checking the branch again; `--base` works as in `check`. A fingerprint found in several files gets one entry per file. An unknown fingerprint is an error. |
| `stop-slop enable` | Turns the hook on in this repository. |
| `stop-slop disable` | Turns the hook off; exceptions and settings stay. |
| `stop-slop rules [--json [--pretty]]` | Lists the rules. `--pretty` works as in `check`. |
| `stop-slop hot <report.json> [--base <ref>] [--json [--pretty]]` | Reads the `report.json` of a [stand](#the-stand) run of this repository and checks the branch as `check` does. Lists the hot functions of the branch with the findings on their lines, and the changed functions that never ran. `--base` and `--pretty` work as in `check`. See [JSON output of `hot`](#json-output-of-hot). |

Flags of `check`:

| Flag | What it does |
|---|---|
| `--base <ref>` | Sets the base. Does not go with `--files` or `--all`, which have no base. |
| `--rev <ref>` | Reads the files from git at this revision instead of the disk, for reviews. Needs `--base` and does not go with `--fix`. |
| `--files <path>...` | Checks only these files, absolute or relative to the current folder. A folder is an error. |
| `--all` | Checks every tracked or untracked file of the repository that git does not ignore. For measurements and one-off cleanups. |
| `--pr <n>` | For reviews. Fetches the head of pull request `<n>` into `refs/stop-slop/pr-<n>` and its base into `origin/<base>`, then checks the head against `origin/<base>`, reading the files from git. When the fetched head is not the one `gh` names, the PR moved meanwhile, and `check` exits with 2. Does not go with `--base`, `--rev`, `--files`, `--all` or `--fix`. |
| `--fix` | Applies the safe fixes, writes the files, then checks them again. |
| `--json` | Prints one line of JSON for an agent. |
| `--pretty` | Indents the JSON by 2 spaces, for a person who saves it to a file and reads it. Needs `--json`. |

Exit codes:

- `0`: nothing found;
- `1`: `check` found something, of any kind; `hot` listed a function, or found no coverage in a report while the branch has files outside tests;
- `2`: the program refused its input: a bad flag, a bad `config.toml` or `ignore.json`, an unknown base or revision, a path of `--files` that does not exist, is a folder or lies outside the repository, a `package.json` or `tsconfig.json` that does not parse, a fingerprint `ignore` does not find, a run outside a git repository, `--pr` without `gh` or for a pull request `gh` does not find, a pull request that moved while `--pr` fetched it, or a `report.json` of `hot` that does not read, has another `schemaVersion` or a head tree this repository does not have;
- `3`: the program itself broke: git failed in a way the program does not expect or did not start, or reading or writing a file failed;
- `130`: Ctrl+C stopped it.

On 2 and 3, stderr says why and ends with a `next: <command>` line for each command that helps, such as `next: stop-slop check --base <ref> --json` when there is no base. With `--json`, `check`, `hot` and `rules` print a [failure line](#json-output-of-check) instead of their JSON.

The hook is the exception. It exits with 2 on errors found and on any failure, because the agent reads a hook's stderr only on 2.

### Text output

```text
base main 76bde5d (ancestor)
skipped src/orders/order.util.ts (already-in-base)
fixed src/markets/markets.service.ts node-protocol x1
src/markets/markets.service.ts:4:12 no-any `any` turns off type checking for this value, so mistakes pass silently. Use a precise type, or `unknown` and narrow it.
```

A finding is `file:line:column rule message`. Lines and columns start at 1, and columns count characters. With `--pr`, a first line such as `pr 123 9f2c4e1 into dev` names the pull request, its head and its base.

### JSON output of `check`

```json
{
  "pr": null,
  "base": { "ref": "origin/feat/markets-cache", "sha": "a1b2c3d", "reason": "pr" },
  "files": ["src/modules/markets/markets.service.ts"],
  "skipped": [{ "file": "src/a.ts", "reason": "already-in-base" }],
  "findings": [
    {
      "rule": "no-process-env",
      "kind": "error",
      "file": "src/modules/markets/markets.service.ts",
      "line": 42, "column": 18, "endLine": 42, "endColumn": 33,
      "function": "MarketsService.fetchPrices",
      "inHunk": false,
      "touched": true,
      "fixable": false,
      "fingerprint": "no-process-env:3f9a1c",
      "detail": { "access": "process.env.API_URL" }
    }
  ],
  "fixed": [{ "rule": "node-protocol", "file": "src/main.ts", "count": 2 }],
  "rules": [
    { "id": "no-process-env", "group": "config", "kind": "error", "message": "`{access}` reads the environment outside a config file: ...", "...": "..." }
  ]
}
```

The program prints it on one line, because indents cost an agent tokens. `--pretty` indents it by 2 spaces for a person. I laid out the example here by hand.

- `pr` is `null` without `--pr`. With it, `pr` has the `number` of the pull request, its full `head` sha, its `base` branch without `origin/` and its `title`.
- `base` has `ref`, a 7-character `sha` and `reason`, which is `flag`, `pr`, `ancestor` or `default`. With `--pr` it is `origin/<base>` with `reason` `pr`. It is `null` for `--files` and `--all`, and then every `inHunk` and `touched` is `null` as well.
- `files` lists the files the program checked, and `skipped` the ones it left out and why.
- `findings` go in order of file, line, column and rule.
  - `kind` is `error` when the rule is sure, `candidate` for a heuristic the agent judges, and `parse-error` when the file did not parse, so no rule ran on it. A `parse-error` finding has `parse-error` as its `rule` too.
  - `function` names the code the finding belongs to: the innermost function or arrow around it, a parameter's function among them, else the class member, else the top-level statement it sits in. It is written as the code names it: `Class.method` (`Class.#field`, `Class.[key]`), an arrow or function expression by its variable, property or assignment target, `(anonymous)` for a function without one, a declaration by its name, and `(top level)` for code outside any declaration, such as an import. A finding in a comment belongs to the code the comment is about: the code before it on its line, else the code after it, such as the method under a doc comment. `function` is `null` for a finding about the file itself: its path (`file-name-pattern`, `kebab-case-dirs`) or a `parse-error`.
  - `touched` is true when the diff changes a line of that code, not counting lines that hold only comments or spaces, or takes out a line of code inside it. A removed line counts as a comment when it starts with `//`, `/*` or `*`. For a finding about the file itself, `touched` is true when the branch adds the file, untracked files too, or renames it.
  - `detail` holds the values of the `{placeholders}` of the rule's `message`, such as the `name` of `file-name-pattern`, and for a `parse-error` its `error`. A finding without such values has no `detail`.
  - `fixable` is true when `--fix` applies the rule's fix at this place. A rule with a fix still leaves a finding without one where it cannot prove the fix is safe; the agent fixes those by hand.
  - `fingerprint`: see [Exceptions](#exceptions).
- `fixed` counts the fixes `--fix` applied, per rule and file.
- `rules` holds the entries of [`rules --json`](#json-output-of-rules) for every rule with a finding and every active `agent` rule, in the order of `rules`, so the agent needs no list of its own. The findings do not repeat a rule's `message`: the message with its placeholders filled from `detail` says why the code is a problem and what to do. `active` here holds for the checked files, with the `package.json` files of the revision `check` read.

`skill/SKILL.md` lists the top-level fields and the fields of `pr`, `base` and a finding. A test holds that list to this output both ways, so a field added or dropped on one side fails it.

On exit 2 and 3, `check --json` prints one line instead, even with `--pretty`, and so do `hot --json` and `rules --json`:

```json
{"failure": {"phase": "base", "cause": "base `dev` is not a commit"}, "next": ["stop-slop check --base <ref> --json"]}
```

- `phase` names the step that failed: `args` (the flags), `repo` (git, the disk, the repository and `--rev`), `base`, `pr` (`gh` and a pull request that moved, under `--pr`), `files` (a path of `--files`), `project` (`package.json` and `tsconfig.json`), `config` (a rule, option or group that `config.toml` names), `state` (the marker, `ignore.json` and a `config.toml` that does not parse), `fingerprint` or `report` (the `report.json` of `hot`).
- `cause` is the error of stderr on one line.
- `next` holds the commands of the `next:` lines and is empty when no command helps.

### JSON output of `hot`

```json
{
  "base": { "ref": "main", "sha": "76bde5d", "reason": "ancestor" },
  "hot": [
    {
      "service": "api", "file": "src/orders/orders.service.ts", "function": "summarize",
      "line": 40, "endLine": 58, "selfMs": 812.0, "totalMs": 2100.0, "entries": 10, "stale": false,
      "findings": [{ "rule": "no-lookup-in-loop", "kind": "candidate", "line": 47, "...": "..." }]
    }
  ],
  "unexercised": [
    { "file": "src/orders/orders.service.ts", "function": "formatRefund", "line": 12, "endLine": 20, "stale": false }
  ],
  "coverage": "collected",
  "rules": [{ "id": "no-lookup-in-loop", "...": "..." }]
}
```

`hot` takes the files of the branch from the disk and needs the report's head tree in this repository; the `root` of the report does not matter. The stand need not be installed.

- `base` is as in `check`.
- `hot` lists the hot functions of the report in files of the branch, the hottest first, one per service that ran it. A function is the one around the place where V8 starts it.
  - `function` is its name. An anonymous function takes the name of the function around it, as a fingerprint does; at the top level it is `null`.
  - `line` and `endLine` are its lines on disk now; `selfMs` and `totalMs` come from the report.
  - `entries` counts how often the coverage repeat entered it, each resumption after an `await` too, all services together; `null` without coverage.
  - `findings` are the findings of `check` on its lines, in the form of `check`.
  - Where the file on disk has no function around that place any more, the row keeps the name and the line of the report: `endLine` is `line`, and `entries` is `null`.
- `unexercised` lists the functions on changed lines of the branch that the coverage repeat never entered, outside test files: the stand runs the app, not its tests. A function inside a listed one is not listed again. The repeat covers only the services with `profile: node`, so a function that only other code runs, such as a script or a service without a profile, is listed too.
- `stale` is true when the file on disk differs from the report's head tree: its lines may have moved since, so run the stand again.
- `coverage` is `collected`, `not-collected` when no service had `x-stand.profile`, or `unknown` when the report has a `coverage-unknown` flag. Without coverage `unexercised` is `null`, since no function can be said to have never run, and a branch with files outside tests exits with 1.
- `rules` holds the entries of `rules --json` for the rules of the findings in `hot`, as in `check`, without the `agent` rules.

Without `--json`, `hot` prints the `base` line of `check` and a line per row, with the service after the name as in the stand's summary and ` stale` at the end where it applies. Without coverage and with files outside tests in the branch, the last line is `coverage: not collected`, or `coverage: unknown: ` and the reason.

```text
base main 76bde5d (ancestor)
812ms src/orders/orders.service.ts:40 summarize api (1 finding, 10 entries)
unexercised src/orders/orders.service.ts:12 formatRefund
```

### JSON output of `rules`

One object per rule:

- `id`, `group` and `description`;
- `kind`: `error`, `candidate` or `agent`;
- `fix`: whether the rule has an automatic fix;
- `files`: `code`, `tests` or `all`, see [File roles](#file-roles);
- `source`: where the rule comes from, see [Sources](#sources);
- `message`: the text of its findings, with `{placeholders}` the rule fills in;
- `agent`: how to judge, fix or check the rule, or `null` when the message says enough;
- `active`: whether the rule runs in this repository, after `disabled`, `enabled` and the group conditions. In a monorepo, a group condition holds when any `package.json` meets it.

`check --json` and `hot --json` carry the entries of the rules they need in their own `rules`.

Without `--json`, `rules` prints `id  kind  fix  description`, one rule per line, and works outside a repository too.

## Exceptions and settings

### Where the files live

The state of a repository sits in `<common>/info/stop-slop/`, where `<common>` is `git rev-parse --git-common-dir`, which is `.git` in a plain clone. Nothing there goes into a commit. All worktrees of a repository share it. In a linked worktree `.git` is a file, not a folder, which is why the program asks git for the path.

| File | What it holds |
|---|---|
| `enabled` | The hook's marker. |
| `ignore.json` | Exceptions. `ignore.json.lock` guards writes to it. |
| `config.toml` | Rules turned off, opt-in groups turned on, and rule options for this repository. |
| `reviews/pr-<n>.md`, `reviews/pr-<n>.json` | The skill's review drafts. |

### Exceptions

`as` and `!` pass when a `// SAFETY: <why>` comment explains them, at the end of the line or in the comment lines right above it. The rules about type assertions and non-null assertions accept it.

Every other exception goes into `ignore.json`:

```json
[
  {
    "fingerprint": "no-boolean-param:8c21e0",
    "rule": "no-boolean-param",
    "file": "src/cache/cache.service.ts",
    "reason": "Signature fixed by the Nest interface it implements.",
    "added": "2026-10-13"
  }
]
```

- A fingerprint is the rule id, a colon and six hex digits of a hash over the finding's source text and the name of the function around it. The line number is not part of it, so code added above does not break an exception. Editing the flagged code, or renaming its function, gives a new fingerprint.
- The function is the innermost named one: a function, a method, or a field, property or variable that holds a function. A finding inside an anonymous callback takes the name of the function outside.
- Two identical places in one function share a fingerprint, so one exception covers both.
- An exception applies to a (file, fingerprint) pair. `ignore` on a pair already in the list replaces its reason and date.

### config.toml

```toml
disabled = ["no-boolean-param"]
enabled = ["performance", "branching"]

[rules.max-nesting]
max = 5
```

- `disabled` turns rules off for this repository. The agent does this when the project's conventions contradict a rule, because the project wins over personal rules.
- `enabled` turns opt-in groups on for this repository. It takes group names, not rule ids (see [Groups](#groups)). `disabled` wins, so a rule it names stays off in an enabled group.
- `[rules.<id>]` overrides the `[options]` of that rule's `rule.toml`. It takes only the keys the rule has.
- An unknown rule, an unknown option, a value of the wrong type, or a group in `enabled` that does not exist or is not opt-in makes every command that reads the file exit with 2. The message names the file and the key, such as `disabled[0]`, `enabled[0]` or `rules.max-nesting.max`, or the line when the TOML itself is wrong.

## Rules

The program has 116 rules in 14 groups: 81 `error`, 29 `candidate` and 6 `agent`. 16 of them fix code themselves. The 12 rules of `performance` and `branching` stay off until `enabled` names them. `stop-slop rules` lists them, and so does [rules.md](rules.md); each one lives in `rules/<group>/<id>/`. The catalog is generic. It names no paths, packages or domains of one project. The ideas I turned down are in [Not checked, on purpose](#not-checked-on-purpose).

### Kinds

- **error.** The rule is exact. After the agent's fixes, none may remain.
- **candidate.** A heuristic. The program finds a candidate and the agent decides whether it is a problem. A false one goes into `ignore.json` with a reason, so a run still ends at zero.
- **agent.** The program cannot check it. The rule is only a `rule.toml`, and the agent checks it from its `agent` text.

A fix never changes what the code does. Where a rule cannot prove that, the finding stays without a fix, with `fixable: false`, and the agent handles it. For example, the naming rules do not rename an exported name, since that reaches other files.

### File roles

`rules/roles.toml` gives a file a role by the suffix before its extension or by a folder on its path. `order.types.ts` has the suffix `types`.

| Role | Suffixes | Folders |
|---|---|---|
| tests | `spec`, `test`, `e2e-spec` | `__tests__` |
| types | `type`, `types`, `interface`, `enum` | |
| constants | `constant`, `constants` | |
| functions | `util`, `mapper`, `config`, `schema` | |

The first role that matches, in this order, wins. A rule's `files` says where it runs: `code` for every file but tests, `tests`, or `all`.

Two more terms the rules use:

- **A file with a class** declares a class at the top level.
- **An own type** comes from the same file, or from an import by a relative path, a `paths` alias of `tsconfig.json`, a workspace package marked `workspace:` in `package.json`, or a `#` import from `imports` in `package.json`. Every other type is a library type.

### Groups

| Group | Rules | What they cover |
|---|---|---|
| `types` | 19 | `any`, type and non-null assertions, needless annotations, optional and nullable fields, enums over string unions, dictionary and `object` types, where types live, reshaping own types |
| `naming` | 15 | file and folder names, case, `I` and `_` prefixes, booleans, acronyms, vague and single-letter names, units in names |
| `functions-classes` | 18 | what goes in a class file and a functions file, parameters, `if` chains and `switch`, singletons, module state, member order, mutation |
| `wandering-data` | 4 | objects that travel through functions that do not use them |
| `imports` | 6 | import order, the `node:` protocol, Node globals, path style, barrels, banned packages |
| `config` | 3 | `process.env`, loose constants, code that special-cases the stand |
| `errors-async` | 8 | empty catches, error classes and codes, `.then`, floating promises, `return await` |
| `logs` | 2 | `console`, secrets in logs |
| `style` | 14 | `let` and `var`, callbacks, nesting, conditions, ternaries, file layout, a few patterns from anti-slop |
| `nestjs` | 8 | `@HttpCode`, request scope, `getOrThrow`, response DTOs, routes, inject tokens |
| `comments` | 4 | `TODO` owners, commented-out code, comments as sentences, comments that retell the code |
| `tests` | 3 | mocks of own modules, test names, test quality |
| `performance` | 9 | opt-in: searches, copies, `shift` and `await` inside loops, `forEach(async ..)`, blocking calls in Nest handlers, `delete` on objects, recursive `process.nextTick`, regexes that backtrack exponentially |
| `branching` | 3 | opt-in: a test an outer test already decided, branches that are the same code, cognitive complexity of a function |

A group can have a `group.toml`:

- `requires-package` runs the group only for files whose package depends on it. The program looks in the nearest `package.json` and every one above it, up to the repository root, the way Node resolves packages from `node_modules`. So the dependencies of a monorepo's root count for every package. `nestjs` requires `@nestjs/core`.
- `files` sets the default `files` of its rules. The `tests` group sets `tests`.
- `opt-in = true` keeps the group off until `enabled` in [config.toml](#configtoml) names it. Only such a group can go into `enabled`.

### Sources

`source` in `rule.toml` says where a rule comes from:

- `personal`: my own coding conventions;
- `direct`: my notes on code taste and design;
- `project`: a work project's conventions, where the rule itself is generic;
- `anti-slop`: ported from [dmmulroy/anti-slop](https://github.com/dmmulroy/anti-slop) under the MIT License, see [THIRD_PARTY.md](../THIRD_PARTY.md).

### Not checked, on purpose

These came up while I built the catalog, and I turned them down:

- **Rules of one project.** A work project's conventions gave rules that only make sense there: its own value types and helpers, folder boundaries and import subpaths, error code ranges, API versioning and Swagger examples, its business domain and its database.
- **What the project's ESLint often has already.** `consistent-type-imports`, which wants `import type`, and `no-wrapper-object-types`, which bans `String` in place of `string`.
- **What ESLint's recommended set checks.** Unreachable code (`no-unreachable`), constant conditions (`no-constant-condition`), an `else if` that repeats an earlier test (`no-dupe-else-if`), a value assigned and never read (`no-useless-assignment`) and `new Promise(async ..)` (`no-async-promise-executor`). A project with ESLint has them on already. Two of them, unreachable code and a value never read, would need a control-flow graph the program does not build.
- **Three anti-slop rules.** `no-unknown-parameters` would forbid `parseUser(input: unknown)`, yet untrusted data should come in as `unknown`, and the code narrows it from there. `require-readable-spacing` is formatting, which belongs to Prettier. `no-runtime-typeof` is stricter than the catalog wants.
- **Rules for a "direct" style.** That style uses plain data and functions instead of class hierarchies. My projects are object-oriented, so these went: a class with only `static` members, a file named for a class role, like `*.service.ts`, with no class in it, `abstract class`, any class in a direct-style project, a wrapper function that only forwards a call, an interface with a single implementation, and a function with a mode of any kind. A boolean flag is still a rule, `no-boolean-param`. From the same notes: `/* */` comments instead of `//`, `/** */` comments inside a function body, and the `// direct:` comment format.
- **Rules that need TypeScript types.** See [Not in v1](#not-in-v1).

## How it works inside

1. Find the base and the files. git calls that do not depend on each other run in parallel.
2. Read the files from the disk, or from git with `git cat-file --batch` for `--rev`.
3. Per file, in parallel with rayon: parse with oxc_parser, build the scopes with oxc_semantic, and run the rules in one walk of the tree. Each rule names the node types it wants.
4. Drop the findings in `ignore.json`.
5. Print the result.

- `no-enum-raw-value` needs the enums of the whole repository. The program builds an index of them before the check, in parallel with the diff, and reads only the files that contain the word `enum`.
- Every other rule works inside one file.
- To tell an own type from a library type, the program reads the file's imports, the `paths` of the nearest `tsconfig.json` and its local `extends`, and the `workspace:` dependencies of `package.json`.
- A fix is a set of text edits by position. Fixes whose edits overlap or touch wait for the next pass. After each pass the program parses and checks the file again, for at most 3 passes. It never fixes a file that does not parse. A fix keeps the file's line endings and permissions.
- `crates/kit/Cargo.toml` pins the exact oxc version, because its API changes between versions.

### A rule folder

```text
rules/
  roles.toml
  <group>/
    group.toml          optional
    <rule-id>/          the folder name is the id, in kebab-case
      rule.toml         always
      check.rs          for error and candidate rules
      valid.ts          no findings; more cases as valid.<name>.ts
      invalid.ts        findings marked with // expect:; more cases as invalid.<name>.ts
      fixed.ts          when, and only when, fix = true
```

`rule.toml`:

```toml
description = "More than 4 levels of nesting"   # the line in `stop-slop rules`
kind = "error"                                  # error | candidate | agent
fix = false
files = "code"                                  # all | code | tests; default from group.toml, else all
source = ["personal"]                           # personal | direct | project | anti-slop
message = "This block sits {depth} levels deep, past the limit of {max}: ..."
agent = """..."""                               # optional; required for kind = "agent"

[options]
max = 4
```

`check.rs` declares a `Check` struct whose fields are the rule's options and implements `kit::Rule` for it, with `on_node` for the node types it lists and `on_file` for the whole file. A rule sees only the `kit` crate. It does no I/O and returns no errors. `rules/clippy.toml` bans the file system, environment, processes, network and console, and `rules/lib.rs` forbids lifting the ban.

`rules/build.rs` reads every folder and generates the list of rules. A broken folder fails the build, and the build lists every problem it finds at once. Among them: bad TOML, an unknown key or source, an id that is not kebab-case, repeats or is a Rust keyword, an `agent` rule with code, fixtures, a fix or no `agent` text, an `error` or `candidate` rule without code or fixtures, `fix = true` without `fixed.ts` or the other way round, an `invalid` fixture without `// expect:` marks or with a mark for another rule, a bad header, a file that does not belong.

### Fixtures

- `// expect: <rule-id>` at the end of a line expects one finding of that rule on that line; `// expect: <rule-id> x2` expects two.
- Header lines at the top of a fixture set what a rule reads around the file. `// @path src/orders/order.response.ts` sets its path for rules that look at roles or names. The default is `src/fixture.ts`. `// @alias @/*`, `// @workspace <package>` and `// @dependency <package>` stand in for a `paths` key of `tsconfig.json` and for dependencies in `package.json`. Each header takes one value; repeat it for more.
- A rule has these test cases:
  - one per `valid*` and `invalid*` fixture;
  - `options`, where the rule builds from its own `[options]`;
  - for a rule with a fix, `fix`, where fixing `invalid.ts` gives `fixed.ts`;
  - for a rule with a fix, `fix-again`, where fixing `fixed.ts` changes nothing.
- `cargo test -p rules -- no-any` runs the cases of one rule.

## Tests

- Each rule has the fixtures above.
- Each crate has its own tests. The base search runs on temporary git repositories: a stack of three branches, a descendant branch, a parent squash-merged into the base.
- CI has two jobs, each on Ubuntu, macOS and Windows. A change in `shared/` runs both.
  - The linter's job, `ci.yml`, runs on changes outside `stand/`: `cargo fmt --check`, `rustfmt --check` on every `check.rs`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace`.
  - The stand's job, `stand.yml`, runs on changes in `stand/` and in `skill/references/`, which the stand builds in: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` in `stand/`, with Node 22 for the tests of the preload.
- `cli` of the stand tests its commands on a fake sandbox, on all three OS.
- The live tests run on real microsandbox VMs with `STOP_SLOP_STAND_LIVE=1`; without it they pass at once.
  - In Linux CI, on KVM, the job `live` of `stand.yml` runs the contract set of the sandbox with the `stand-guest` of the commit, then both cases of `cli` on the pinned release. The whole case asks 3 vCPUs, so the job sets `host_share = 80`, and the runner's 4 CPUs give 3.
  - On Windows Claude runs the whole live set before each push of the stand; on macOS I do. On Windows they run from the test binary that `cargo test --no-run` builds, never under `cargo test` or `cargo run`: cargo keeps its children in a job without breakaway, and a VM from a snapshot cannot start there.
- I measure speed by hand with `check --all` on a real repository, read-only.

## Speed

The targets hold for Windows, Linux and macOS. I measure them on Windows, where starting a process costs the most. `gh pr view` does not count. It is network, 0.3 to 1.5 seconds, and `--base` skips it.

| What | Target | Last measured |
|---|---|---|
| a diff of 30 files | under 100 ms | 99 to 100 ms on 34 files |
| `check --all` on ~1,050 `.ts` files, ~127,000 lines | under 1 s | 126 ms |
| the hook on one file | under 50 ms | 37 ms |

I measured this in October 2026 with all 103 rules, a release build, Windows 11 and git 2.47, on a clone of a NestJS monorepo after `git gc` and `git update-index --refresh`. The rules take 16 to 18 ms of that for all files; the rest is git and process start. Without the refresh, a fresh clone runs the diff in about 160 ms, because git rechecks the files its index cannot trust.

## The stand

### What it does

`stop-slop-stand run` reads the stand file and takes the work tree as a git tree: the files on disk, tracked or not, without the ones git ignores. It puts that tree on a disk of its own in a VM, and the setup services install and build there. Then the workload runs `repeat` times, 3 by default, each time on a fresh copy of the state after setup, and the stand measures each repeat. With `--before` it does the same for the tree it compares with, and the repeats of the two versions take turns. It prints a summary or one line of JSON and the path of the report, and destroys every VM. The work tree and the index stay as they were; in `.git` the stand adds only git objects and the ref of `snapshot`.

A session is one stand file in one work tree. It holds the seal of the recipe, the runs with their reports, the kept snapshots and the setup cache.

The stand measures only my own branch. It never runs the code of a colleague's pull request.

### Commands

| Command | What it does |
|---|---|
| `stop-slop-stand run [--before session\|<ref>] [--json [--pretty]]` | Measures the head, and with `--before` the tree it changed. `session` is the snapshot of this work tree; a `<ref>` stands for `git merge-base HEAD <ref>`. |
| `stop-slop-stand run --before <..> --reseal --reason "<text>"` | Seals the changed recipe again and starts the comparison over. |
| `stop-slop-stand run --dry-run [--before ..] [--json]` | Checks the stand file, the seal, the budget and the trees as `run` does. Then it prints `cached <digest>` or `to download` for each image, and `setup <side> cached <run>` or `to build` for each version. No VM starts, and nothing is pulled, sealed or dropped: `would destroy`, `would drop` and `would forget` lines name what the sweep and a low disk would take. With `--json` the line holds no facts; the exit code tells. |
| `stop-slop-stand snapshot` | Records the work tree, uncommitted and untracked files too, as "before" of this session, in `refs/stop-slop/stand/before/<work tree key>`. Prints its sha, and `replaced <old>` when there was one. The index and the files stay. |
| `stop-slop-stand inspect <run> <side> <service> -- <argv>` | Runs `argv` in a throwaway copy of the state the kept run ended with, see [inspect](#inspect). |
| `stop-slop-stand accept <measure> --allow <bound> --reason "<text>"` | Accepts the shift of a measure after the fact, see [The declared cost](#the-declared-cost). |
| `stop-slop-stand guide [topic]` | Prints the guide for agents. Without a topic: the shortest path, what to do on each exit code and the list of topics. With one: the topic `recipe`, `probes`, `expect`, `line` or `inspect`. |
| `stop-slop-stand init` | Writes a stand file to start from, with comments, and prints its path. A stand file that is there already is exit 2. |
| `stop-slop-stand clean [--all]` | Deletes every VM, then every snapshot, then the images, then the data of every session: runs and reports, seals, acceptances and the setup cache. It also drops the snapshot ref of this work tree. The runtime of microsandbox, `stand-guest` and the journals of the sessions stay. It takes the sessions of every work tree that uses the stand folder. `--all` deletes the whole stand folder but its lock, without asking microsandbox first: a VM that a crashed run left keeps running until its time is up. |

`--help` holds at most 20 lines and ends with ``agents: run `guide` ``. The text of `guide` is `skill/references/guide.md` and `probes.md`, built into the program.

`run`, `inspect`, `accept` and `clean` take the lock of the stand folder, one at a time. While another command holds it, a command prints `waiting for the stand` and waits as long as a whole run of its stand file may take, an hour for `clean` without a stand file that loads, then exits with 2. Ctrl+C stops a run: the VMs go, what it collected stays in the run folder, and it exits with 130. A second Ctrl+C leaves at once, without a line, and the next start sweeps what is left.

Exit codes of `run`:

- `0`: the head passed every repeat. A failed "before", diffs, grades, `needs`, flags and failed probes are facts, not failures.
- `1`: the head failed: a setup service or the workload exited with another code than 0, a timeout, a long-running service that died, a healthcheck that never passed. No report is written. `next` names an `inspect` of each snapshot the failure kept, or `guide recipe`.
- `2`: the stand refused its input and measured nothing: no stand file, a stand file or a `stand.toml` it refuses, a recipe changed since its seal, the budget, no snapshot for `session`, an unknown ref, no merge-base, a "before" tree equal to the head's, an image the registry does not know, a result in `/stand/out` that is not a plain file or is over 64 MiB, a run outside a git work tree, a bad flag, or waiting too long for another run.
- `3`: the stand itself broke: microsandbox, the registry, the download or the sha256 of `stand-guest`, git, the disk, reading the host or the report.
- `130`: Ctrl+C.

On 2 and 3 stderr says why and ends with a `next: <command>` line for each command that helps. The other commands exit 0, 2, 3 or 130 the same way; `inspect` exits 0 whatever the code of `argv`.

### The stand file

`<git common>/info/stop-slop-stand/compose.yaml`, with the harness beside it. All work trees of the repository share it. The folder is the recipe: the stand file and every file next to it. It lives outside the repository, so nothing of it goes into the diff. It is plain Compose, a subset of it, plus `x-stand`: `STAND_REPO=<repo> docker compose -f <file> up` runs it too.

- The top level takes `services`, `volumes`, `name`, `x-stand` and `x-*`. A service takes `image`, `command`, `entrypoint`, `environment`, `working_dir`, `volumes`, `expose`, `depends_on`, `healthcheck`, `stop_signal`, `stop_grace_period`, `deploy.resources.limits`, `deploy.replicas`, `x-stand` and `x-*`. Any other key is refused with its path and line; `build:` and `ports` are not in v1.
- `deploy.resources.limits.cpus` and `.memory` are required. A VM gets whole vCPUs: `cpus: "0.5"` takes one.
- A service another one calls needs `expose`; without it the name resolves but does not answer, and the report flags `missing-expose`.
- A service that another one waits for with `service_completed_successfully` is a setup service: it runs once per version, before the repeats. `x-stand.workload` names the workload: its exit ends a repeat, and nothing waits for it. Both run one replica. The other services are long-running. `service_healthy` needs a `healthcheck`.
- A service name has no `.` and is neither `stand` nor `volume`.
- `stop_signal` defaults to the image's `STOPSIGNAL`, then `SIGTERM`, as in Docker; `stop_grace_period` to `10s`. Every repeat ends with that clean stop of each long-running service, and so does setup before each snapshot.
- Mounts, in the short form only:
  - `${STAND_REPO}:/w/app` mounts the tree of the measured version at `/w/app` of a disk of its own, the version disk at `/w`: always that target, without `:ro` or a subfolder. No other `$` substitution and no host path.
  - A relative source is the harness, the same for both versions: a folder of the recipe, with `:ro`.
  - A named volume is an ext4 disk of the one service that mounts it, without `:ro`; that service runs one replica and does not mount `${STAND_REPO}`.
  - `/stand/` and `/w/` are the stand's: no target there but `/w/app`. No target is `/`, and no two mounts share one.
- The prepare service is the setup service with `${STAND_REPO}` that every other service with `${STAND_REPO}` waits for. It installs and builds on the version disk, and the VM of each other service with `${STAND_REPO}` starts from its snapshot. So those services use its `image`, written the same, and it mounts nothing but `${STAND_REPO}`. Without a prepare service each VM with `${STAND_REPO}` unpacks the tree itself. Keep the store and the cache of the package manager on `/w`, outside `/w/app`, never in a bind.
- `x-stand` at the top level: `workload`, `repeat` (3), `setup_timeout` (`10m`), `timeout` per repeat (`5m`), `probes`, `expect`, `fault`, `second_pass` (`false`). `x-stand` of a service: `profile: node` and `network: public`. `x-` keys elsewhere are refused.
- `profile: node` profiles the Node processes of the service: their hot functions in each measured repeat, and their coverage in one more repeat of the head, outside the stats.
- `x-stand.probes.<name>: {in, at, run, key, ignore}`: a command that runs in the first replica of a long-running service, at `after-setup`, `after-workload` or `on-failure`, under 30 seconds, as the image's user, with no network beyond its own VM. Its stdout is compared as `probe.<name>.jsonl`. `guide probes` has the keys and examples.
- `x-stand.expect`: `cost`, a list of `{measure, change, reason}`, and `unchanged`, a list of `{file, reason}`, which needs `second_pass: true`. `change` is `none`, `+X%`, `-X%`, `down` or `unknown`, without quotes, X a whole percent from 5.
- `x-stand.fault: {target: <service>:<port>, mode: down}` or `{target, mode: slow, delay}` adds 2 repeats per version outside the stats, where that target refuses or delays its connections. `x-stand.second_pass: true` adds one repeat per version that runs the workload twice on one state. Their results are facts; a failure there does not change the exit code.
- Every repeat starts from the state after setup. The workload writes `*.jsonl` into `/stand/out`, which is fresh for each repeat; with `--before` each file is compared with the same file of "before", record by record. Each `*.jsonl` there, a probe's output too, is a plain file of at most 64 MiB: a link, a FIFO, a folder or a bigger file of that name is exit 2.

The stand runs `id`, `tar` and `du` inside the guests, so an image without them, such as a distroless one, cannot run in it.

### The seal

The first `run --before` of a session seals the recipe once it fits the budget and its images are pulled: a copy of its folder goes into the session, and every run with `--before` reads that copy. A changed, new or deleted file of the live folder is exit 2 that names it, with the `--reseal` command in `next`. `--reseal --reason` seals it again and writes the reason into the session's journal. The line of every later run with `--before` shows the journal in `seal`, together with the `max_tolerance` in effect. The journal outlives `clean`, so the next seal shows `cleaned` too. `run` without `--before` reads the live folder and seals nothing: that is free exploration.

### The declared cost

With `--before` each measure gets a grade against its entry in `x-stand.expect.cost`: `met`, `exceeded`, `surprise` or `unmeasurable`; a measure without an entry is graded as `none`. `unknown` gets no grade and goes into `needs`, a list of `{measure, reason}` for a person. An `unmeasurable` measure without an entry stays quiet and is counted in `unmeasurable`. Apart from its grade, a measure has a verdict against "before" and against the previous run: `improved` or `regressed` where less is better, `changed` for connections and bytes, which have no better side, `within-noise` or `no-baseline`.

`accept <measure> --allow <bound> --reason "<text>"` adds an acceptance for the head tree of the last passed run: `<bound>` is `none`, `+X%`, `-X%` or `down`. Acceptances only add up. The last one for the head tree of a run takes the place of the declared entry: the line shows it with `postHoc`, and `acceptances` counts them all. Once the head tree changes, no old acceptance holds, and the line marks the measure `stale`. `unknown`, a bound wider than `max_tolerance`, a reason without a bound or a bound without a reason, a measure the last passed run does not have, and no passed run yet are exit 2.

Measures are named `<subject>.<metric>`: `stand.wall`, `stand.hot-owned`, `stand.hot-collateral`, `<service>.cpu`, `.memory`, `.error-lines`, `.shutdown`, `<service>:<port>.connections`, `.bytes`, `.http-5xx-share`, `volume:<name>.bytes`. The stand measures every one of them itself, so each has `src: stand`: a number of the harness never becomes a measure.

### The `--json` line

One line on every exit code, with only the exceptions: `run`, `report`, `seal`, `attempts`, `comparisons`, `measures`, `behaviour`, `fault`, `secondPass`, `flags`, `needs`, `failure`, `quiet`, `unmeasurable` and `next`. `quiet` counts what passed; the numbers stay in `report.json`. Runs of the same head tree, "before" tree and recipe pool their samples, and `attempts` counts them. `next` holds commands ready to paste, such as the `accept` of a `surprise`; every failure and refusal has one, Ctrl+C none. A ref of `--before` that a shell would read otherwise comes in single quotes, which a POSIX shell and PowerShell both take as typed. `--pretty` indents it by 2 spaces. `guide line` describes every field and flag; a test holds the topic and the line to each other.

`report.json` (`schemaVersion` 1) lies in the run folder and is written only for a passed run. It follows the JSON of `check`: camelCase keys, kebab-case values, paths from the root with `/`, full oids. It is for people and tools; the agent reads the line, and `stop-slop hot` reads the report for the hot functions of the branch.

### The setup cache

A setup that passed stays for the next runs of its session: the version snapshot, the snapshots of the setup volumes, the volume sizes and the `after-setup` outputs. A run uses it when its key matches: the version's tree, the `digest` of the recipe the run reads, the digest of each image after `pull`, the version of `stand-guest` and of the stand. There are two slots, `before` and `head`, one entry each; a run without `--before` changes only `head`, so a second `run --before session` without a reseal takes the setup of "before" from the cache. `setup.cached` of `head` and `before` in `report.json` names the run that built it, `null` when this run did; the summary prints it, the line does not. `clean` and the TTL of the session delete the cache, and on a low disk it goes before the kept snapshots of its session.

A run whose pool already holds a passed run repeats only the measured repeats: the results of the coverage, fault and second-pass repeats come from the last passed run of the pool, with `from: "<run>"` in `report.json` and in the line. Such a repeat runs again when that run has no result for it, or when a fault category came out unknown there.

### inspect

`inspect <run> <side> <service> -- <argv>` takes the kept run of the session: the last run that kept snapshots, of the services with a volume, at the end of its last measured repeat or at the failure. A newer kept run drops the snapshots of the one before. `inspect` starts the snapshot of `<service>` of `<side>` (`head` or `before`) in a throwaway VM without neighbours and without network, with the command of its service. It runs `argv` under 60 seconds, prints `exit <code>` or `timeout`, at most 40 lines and 64 KiB of its stdout and the path of the whole output, and destroys the VM. When `argv` left something other than a regular file at that path, such as a link, one line says so and nothing of it is printed. A write lands in that copy only. A service that does not start alone is exit 2 with the reason. It gives no verdict.

### stand.toml and the folders

| Key of `stand.toml` | Default | What it does |
|---|---|---|
| `host_share` | 50 | The share of the host's CPUs and memory the stand may take, 1 to 80. |
| `max_tolerance` | 25 | The widest `+X%` or `-X%` of `expect` and `accept`, 1 to 1000. Above 25 the line flags `wide-tolerance`. |

A bad `stand.toml` is exit 2 that names the file and the key. It lies in the config folder of the OS, in `stop-slop-stand/stand.toml`.

The stand folder is `STOP_SLOP_STAND_HOME`, or `%LOCALAPPDATA%\stop-slop-stand` on Windows, `~/.local/share/stop-slop-stand` on Linux and `~/.stop-slop-stand` on macOS. microsandbox keeps Unix sockets under it, and a socket path holds 104 bytes on macOS and 108 on Linux, so on those keep `STOP_SLOP_STAND_HOME` under about 45 characters. `STOP_SLOP_STAND_GUEST` names a build of `stand-guest` to use instead of the pinned release, unchecked; it is for developing the stand.

The budget: setup goes one version after the other, and so do the repeats, so the stand asks for its peak. That is the larger of setup (the setup services and the long-running ones they wait for) and a repeat (the long-running services and the workload), for CPUs and memory each: the limits times the replicas, CPUs as whole vCPUs. The stand refuses with the numbers when the peak does not fit: CPUs over `host_share` of the logical CPUs, or memory over the lower of `host_share` of all memory and the free memory less 2 GiB. It also refuses when the disk of the stand folder has less than 20 GiB free, once it has dropped the setup cache and kept snapshots of the other sessions, then of its own; a stand that the CPUs or the memory refuse drops nothing. A session lives 3 days after its last use, with its runs, reports, kept snapshots and setup cache.

### Two networks

The host needs the network to download the runtime of microsandbox, the images and `stand-guest`. The guests have none to the outside: a service gets it only with `x-stand: { network: public }`, so a dependency I did not stub fails at once. Each guest reaches the other services by name.

## Repository layout

```text
stop-slop/
  Cargo.toml        the linter's workspace
  .gitattributes    *.ts text eol=lf, so fixtures compare the same on Windows
  .github/workflows/
    ci.yml          the linter's checks
    stand.yml       the stand's checks and its live tests
    guest-release.yml  builds stand-guest for a guest-v* tag and publishes it
  crates/
    cli/            commands, flags, text and JSON output, the hook
    repo/           git and the disk: a snapshot of files (the working tree or a revision)
    scope/          the base and the list of files
    project/        package.json, tsconfig.json, the enum index
    state/          <common>/info/stop-slop: the marker, ignore.json, config.toml
    engine/         parsing, one walk of the tree, running rules, fixes
    kit/            what a rule sees: the tree, the file's role, options, report and fix, shared helpers
    catalog/        for the build and the rule tests, not in the program: reads and checks rules/, writes the list of rules
  shared/           crates that take nothing from the workspace, so a second workspace can use them too
    gitcmd/         runs git: a cleaned environment, LC_ALL=C, another try when the index is busy
    testkit/        tests only: a temporary git repository
    fixtures/       report.json as the stand writes it, held to the code by a test of report and read by the tests of hot
  stand/            the stand, a second workspace with its own Cargo.toml and Cargo.lock
    crates/
      cli/          the command stop-slop-stand: flags, the call order of a run, the seal, the cache slots, the sweep, guide and init
      spec/         the stand file: the allowed subset of Compose, x-stand, the recipe and its digest, the demand
      budget/       the host's CPUs, memory and free disk against what a stand asks for
      source/       the work tree, its snapshot and "before" as git trees, and their tar files
      run/          one run in VMs: setup and its cache entries, repeats, probes, the clean stop, kept snapshots, inspect
      sandbox/      the port to the VMs, the microsandbox adapter and a fake for tests
      guest/        stand-guest, the helper inside each VM: names, counters, faults, stops, probes, trim
      profile/      the Node preload, which writes CPU profiles or coverage, and their reading
      report/       measures, verdicts, grades, flags and diffs: report.json, the line and the summary
      store/        the stand folder: lock, sessions, runs, the seal, acceptances, the setup cache
  rules/            the rules, a crate of their own: Cargo.toml, build.rs, lib.rs, fixtures.rs, clippy.toml, roles.toml
    <group>/<rule-id>/
  skill/            the skill: SKILL.md, fix.md, stand.md, review.md, send.md, agents/openai.yaml
    references/     guide.md and probes.md, the stand's guide, built into stop-slop-stand
  docs/
    spec.md         this file
    rules.md        every rule, by group
  README.md
  THIRD_PARTY.md
```

Each part is its own crate, so the compiler holds the boundaries between them, and rules depend only on `kit`.

## Install

- Windows, Linux or macOS, with git 2.41 or newer, because the ancestor search needs `for-each-ref %(ahead-behind)`.
- `gh`, signed in, for the base of a pull request and for reviews. Without it `check` still works and skips that step, but `check --pr` exits with 2.
- Rust 1.97 or newer.

```sh
git clone https://github.com/MarsianinUA/stop-slop
cd stop-slop
cargo install --path crates/cli --locked
```

This puts `stop-slop`, or `stop-slop.exe` on Windows, into `~/.cargo/bin`, which rustup puts on `PATH`.

The stand installs the same way:

```sh
cargo install --path stand/crates/cli --locked
```

It needs a hypervisor: WHP on Windows (Windows Hypervisor Platform, a Windows feature), KVM on Linux, Apple silicon on macOS. On the first run it downloads the runtime of microsandbox and the pinned `stand-guest`, checked by its sha256.

The skill goes where the agent keeps its other skills. Link `skill/` there, or copy it:

```sh
# Linux, macOS
mkdir -p ~/.claude/skills
ln -s "$PWD/skill" ~/.claude/skills/stop-slop
```

```powershell
# Windows
New-Item -ItemType Junction -Path "$HOME\.claude\skills\stop-slop" -Target "$PWD\skill"
```

Reviews also use the `unslop` writing skill, which is not part of this repository.

For the hook, add the `PostToolUse` entry from [The hook](#the-hook) to `~/.claude/settings.json` once, then run `stop-slop enable` in each repository where you want it.

## Not in v1

- Rules that need TypeScript types: `await-thenable`, `no-floating-promises` across files. The `no-floating-promise` rule covers calls of async functions declared in the same file.
- JavaScript, `.vue` and other languages.
- Running in CI.
- GitLab and other hosts. GitHub only.
- The stand does not run a colleague's pull request: their code would run on my machine.
- In the stand: `ports` in the stand file, an MCP server, recorders built in for particular databases, replicas of a service with a volume, a service with both `${STAND_REPO}` and a volume, and services of different images on the tree of a prepare service.
- Checks a stand could do but would do badly, so it does not: soak runs and the slope of RSS, latency percentiles, event-loop delay, GC numbers, overload ramps, deterministic simulation, `kill -9` and recovery, security scanners, secrets by regex, migration locks, down migrations, fake clocks, runs of mixed versions. A check gets in only when it tells right code from wrong, repeats or has measurable noise, runs in minutes, gives the agent something to act on, is not cheaper as a rule of `stop-slop`, and gives no false calm on stub data.
- More than one run at a time per stand folder, which by default is one per machine.
