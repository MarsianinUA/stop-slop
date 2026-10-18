# Fix the branch

Fix the findings in the files of the user's branch, with no approval step. The run is done when `check` is **clean**: no `error` left, and every `candidate` either fixed or ignored with a reason.

1. `git fetch`, so the base is fresh. When `git remote` is empty or the fetch fails, go on and say so in the report.
2. `stop-slop check --json`. Keep this first list for the report, in a temp file outside the repository: it tells the user's own lines (`inHunk: true`) from cleanup in old code (`inHunk: false`). Read `stop-slop rules --json` and the project conventions (SKILL.md).
3. `stop-slop check --fix --json`: the program applies its safe fixes and prints what is left.
4. Fix every `error`, as its `message` says and its rule's `agent` text advises. Fix a `candidate` when its `agent` hint says it is a real problem; keep the others for step 6. Fix a `parse-error` when the branch broke the syntax, otherwise report it.
   - When a fix needs something the repository lacks (a logger, a config service), make the smallest change that meets the rule and say so in the report.
   - Moving or renaming a file or an exported name reaches other files: update every import and use of it, and note those files for the report.
5. Check each `agent` rule with `active: true`, as its `agent` text says, in the branch files its `files` covers (the latest `check`'s `files` and the files you added). Fix what it finds.
6. `stop-slop check --fix --json` again. While an `error` or an undecided `candidate` is left, go back to step 4. Then ignore each candidate you judged fine: `stop-slop ignore <fingerprint> --reason "<why it is fine here, in English>"`. Ignore last, because an edit in the same function changes the fingerprint. A final `check --json` is clean.
7. Run the project's formatter on the changed files, if the project has one (`prettier`, `biome` or `dprint` in `package.json` or a config file), through the package manager its lockfile names (npm without one). Then `check --json` again: it stays clean.
8. Run the `typecheck` and `lint` scripts of `package.json` the same way, if they exist. Fix what your edits broke, and note the result.
9. Leave the commit to the user. Report:
   - the base and why the program picked it;
   - what you fixed in the user's own lines, by rule;
   - cleanup: what changed in old code (the `inHunk: false` findings of step 2), by file and line, so the user can commit it apart (`git add -p` where a file holds both kinds);
   - files touched by moved or renamed files and exported names;
   - every ignored finding with its reason, and every rule a convention disabled;
   - the formatter, `typecheck` and `lint` results.
