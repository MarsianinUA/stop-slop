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
   - It makes no network calls of its own. It runs `git`, and `gh` to look up the base of a pull request.
2. **The skill** in `skill/`, for a coding agent. The agent runs the program, judges the findings, fixes what the program cannot, and writes review comments.
3. **The agent hook.** It checks a file right after the agent edits it, in repositories where you ran `stop-slop enable`.

## Modes

The skill has three branches, and each run takes one: fix the user's branch, review a colleague's PR, or send a review draft the user approved. The user names stop-slop in the request: "fix my branch with stop-slop", "review PR 123 with stop-slop", then "send it".

### Fix your branch

The agent fixes everything at once, with no approval step. The run ends when `check` is clean, with no `error` left and every `candidate` either fixed or ignored with a reason. A `performance` or `branching` fix the project's tests reject is the exception: it is reverted, and its finding goes to the report.

1. `git fetch`, so the base is fresh. With no remote, or when the fetch fails, the run goes on and the report says so.
2. `stop-slop check --json`. The agent keeps this first list for the report, outside the repository. Its `inHunk` tells the user's own lines from cleanup in old code.
3. The agent reads `stop-slop rules --json` and the project's conventions, see [Project conventions win](#project-conventions-win).
4. `stop-slop check --fix --json`. The program applies its safe fixes and prints what is left.
5. The agent goes through the findings:
   - it fixes an `error` as its `message` says and the rule's `agent` text advises;
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

### Review a colleague's PR

The working tree stays as it is. The run ends on a draft that the user edits.

1. `gh pr view <n> --json baseRefName,headRefOid,title`.
2. Fetch the PR head and its base without a checkout: `git fetch origin +refs/pull/<n>/head:refs/stop-slop/pr-<n> +refs/heads/<base>:refs/remotes/origin/<base>`. When the fetched head is not `headRefOid`, the PR has moved, and the run starts again.
3. `stop-slop check --rev refs/stop-slop/pr-<n> --base origin/<base> --json`. The program reads the files straight from git.
4. The agent reads `stop-slop rules --json`, and the project's conventions as they are at the PR head. `active` in `rules --json` still comes from the `package.json` files of the working tree, not of the PR.
5. The agent drops:
   - false positives: a candidate its hint clears, or an error the code proves wrong;
   - what the project's conventions allow;
   - what is not the author's: an old finding with `inHunk: false` in a function the PR left alone, unless the PR made it worse. The PR touched a function when its diff changes a line of the function's code, not only a comment.

   When most of the codebase follows a habit a rule flags and no convention mentions it, the agent keeps the finding and points it out to the user.
6. The agent checks each active `agent` rule in what the PR adds or changes.
7. The agent writes a comment for every finding left, except a `parse-error`, which goes to the user. Findings of one rule in one function share a comment.
8. The agent writes the draft to `<common>/info/stop-slop/reviews/pr-<n>.md`, see [Where the files live](#where-the-files-live). The user deletes the blocks they disagree with, edits the text, and tells the agent to send.

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

Each finding gets `inHunk`, which says whether the branch changed its first line.

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

Flags of `check`:

| Flag | What it does |
|---|---|
| `--base <ref>` | Sets the base. Does not go with `--files` or `--all`, which have no base. |
| `--rev <ref>` | Reads the files from git at this revision instead of the disk, for reviews. Needs `--base` and does not go with `--fix`. |
| `--files <path>...` | Checks only these files, absolute or relative to the current folder. A folder is an error. |
| `--all` | Checks every tracked or untracked file of the repository that git does not ignore. For measurements and one-off cleanups. |
| `--fix` | Applies the safe fixes, writes the files, then checks them again. |
| `--json` | Prints one line of JSON for an agent. |
| `--pretty` | Indents the JSON by 2 spaces, for a person who saves it to a file and reads it. Needs `--json`. |

Exit codes:

- `0`: nothing found;
- `1`: `check` found something, of any kind;
- `2`: the program refused its input: a bad flag, a bad `config.toml` or `ignore.json`, an unknown base or revision, a path of `--files` that does not exist, is a folder or lies outside the repository, a `package.json` or `tsconfig.json` that does not parse, a fingerprint `ignore` does not find, or a run outside a git repository;
- `3`: the program itself broke: git failed in a way the program does not expect or did not start, or reading or writing a file failed;
- `130`: Ctrl+C stopped it.

On 2 and 3, stderr says why and ends with a `next: <command>` line for each command that helps, such as `next: stop-slop check --base <ref> --json` when there is no base. With `--json`, `check` and `rules` print a [failure line](#json-output-of-check) instead of their JSON.

The hook is the exception. It exits with 2 on errors found and on any failure, because the agent reads a hook's stderr only on 2.

### Text output

```text
base main 76bde5d (ancestor)
skipped src/orders/order.util.ts (already-in-base)
fixed src/markets/markets.service.ts node-protocol x1
src/markets/markets.service.ts:4:12 no-any `any` turns off type checking for this value, so mistakes pass silently. Use a precise type, or `unknown` and narrow it.
```

A finding is `file:line:column rule message`. Lines and columns start at 1, and columns count characters.

### JSON output of `check`

```json
{
  "base": { "ref": "origin/feat/markets-cache", "sha": "a1b2c3d", "reason": "pr" },
  "files": ["src/modules/markets/markets.service.ts"],
  "skipped": [{ "file": "src/a.ts", "reason": "already-in-base" }],
  "findings": [
    {
      "rule": "no-process-env",
      "kind": "error",
      "file": "src/modules/markets/markets.service.ts",
      "line": 42, "column": 18, "endLine": 42, "endColumn": 33,
      "message": "`process.env.API_URL` reads the environment outside a config file: ...",
      "inHunk": true,
      "fixable": false,
      "fingerprint": "no-process-env:3f9a1c"
    }
  ],
  "fixed": [{ "rule": "node-protocol", "file": "src/main.ts", "count": 2 }]
}
```

The program prints it on one line, because indents cost an agent tokens. `--pretty` indents it by 2 spaces for a person. I laid out the example here by hand.

- `base` has `ref`, a 7-character `sha` and `reason`, which is `flag`, `pr`, `ancestor` or `default`. It is `null` for `--files` and `--all`, and then every `inHunk` is `null` as well.
- `files` lists the files the program checked, and `skipped` the ones it left out and why.
- `findings` go in order of file, line, column and rule.
  - `kind` is `error` when the rule is sure, `candidate` for a heuristic the agent judges, and `parse-error` when the file did not parse, so no rule ran on it. A `parse-error` finding has `parse-error` as its `rule` too.
  - `message` says why the code is a problem and what to do, not only what the rule found.
  - `fixable` is true when `--fix` applies the rule's fix at this place. A rule with a fix still leaves a finding without one where it cannot prove the fix is safe; the agent fixes those by hand.
  - `fingerprint`: see [Exceptions](#exceptions).
- `fixed` counts the fixes `--fix` applied, per rule and file.

`skill/SKILL.md` lists the top-level fields, the fields of `base` and those of a finding. A test holds that list to this output both ways, so a field added or dropped on one side fails it.

On exit 2 and 3, `check --json` prints one line instead, even with `--pretty`, and so does `rules --json`:

```json
{"failure": {"phase": "base", "cause": "base `dev` is not a commit"}, "next": ["stop-slop check --base <ref> --json"]}
```

- `phase` names the step that failed: `args` (the flags), `repo` (git, the disk, the repository and `--rev`), `base`, `files` (a path of `--files`), `project` (`package.json` and `tsconfig.json`), `config` (a rule, option or group that `config.toml` names), `state` (the marker, `ignore.json` and a `config.toml` that does not parse) or `fingerprint`.
- `cause` is the error of stderr on one line.
- `next` holds the commands of the `next:` lines and is empty when no command helps.

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

The program has 115 rules in 14 groups: 80 `error`, 29 `candidate` and 6 `agent`. 16 of them fix code themselves. The 12 rules of `performance` and `branching` stay off until `enabled` names them. `stop-slop rules` lists them, and so does [rules.md](rules.md); each one lives in `rules/<group>/<id>/`. The catalog is generic. It names no paths, packages or domains of one project. The ideas I turned down are in [Not checked, on purpose](#not-checked-on-purpose).

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
| `config` | 2 | `process.env` and loose constants |
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
- CI runs `cargo fmt --check`, `rustfmt --check` on every `check.rs`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` on Ubuntu, macOS and Windows.
- I measure speed by hand with `check --all` on a real repository, read-only.

## Speed

The targets hold for Windows, Linux and macOS. I measure them on Windows, where starting a process costs the most. `gh pr view` does not count. It is network, 0.3 to 1.5 seconds, and `--base` skips it.

| What | Target | Last measured |
|---|---|---|
| a diff of 30 files | under 100 ms | 99 to 100 ms on 34 files |
| `check --all` on ~1,050 `.ts` files, ~127,000 lines | under 1 s | 126 ms |
| the hook on one file | under 50 ms | 37 ms |

I measured this in October 2026 with all 103 rules, a release build, Windows 11 and git 2.47, on a clone of a NestJS monorepo after `git gc` and `git update-index --refresh`. The rules take 16 to 18 ms of that for all files; the rest is git and process start. Without the refresh, a fresh clone runs the diff in about 160 ms, because git rechecks the files its index cannot trust.

## Repository layout

```text
stop-slop/
  Cargo.toml        the workspace
  .gitattributes    *.ts text eol=lf, so fixtures compare the same on Windows
  .github/workflows/ci.yml
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
  rules/            the rules, a crate of their own: Cargo.toml, build.rs, lib.rs, fixtures.rs, clippy.toml, roles.toml
    <group>/<rule-id>/
  skill/            the skill: SKILL.md, fix.md, review.md, send.md, agents/openai.yaml
  docs/
    spec.md         this file
    rules.md        every rule, by group
  README.md
  THIRD_PARTY.md
```

Each part is its own crate, so the compiler holds the boundaries between them, and rules depend only on `kit`.

## Install

- Windows, Linux or macOS, with git 2.41 or newer, because the ancestor search needs `for-each-ref %(ahead-behind)`.
- `gh`, signed in, for the base of a pull request and for reviews. Without it `check` still works and skips that step.
- Rust 1.97 or newer.

```sh
git clone https://github.com/MarsianinUA/stop-slop
cd stop-slop
cargo install --path crates/cli --locked
```

This puts `stop-slop`, or `stop-slop.exe` on Windows, into `~/.cargo/bin`, which rustup puts on `PATH`.

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
