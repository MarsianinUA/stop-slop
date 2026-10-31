# Fix the branch

Fix the findings in the files of the user's branch, with no approval step. The run is done when `check` is **clean**: no `error` left, and every `candidate` either fixed or ignored with a reason. Two exceptions stay open, and the report names them:

- a `performance` or `branching` fix the project's tests reject: step 9 reverts it;
- old code (`inHunk: false`) in a file that only your own edit brought into `check`, such as an import that a rename or a moved type updated: old code is cleaned only in the files of the first `check` (step 2), a file you renamed among them.

The run also measures its fixes on the stand ([stand.md](stand.md)) when the user asks for that, or when the repository has a stand file, `<common>/info/stop-slop-stand/compose.yaml`: the user set the stand up for it. Steps 3, 10 and 11 say where, and the run is done when those steps are done too.

1. `git fetch`, so the base is fresh. When `git remote` is empty or the fetch fails, go on and say so in the report.
2. `stop-slop check --json`. Keep this first list for the report, in a temp file outside the repository: it tells the user's own lines (`inHunk: true`) from cleanup in old code (`inHunk: false`). Read `stop-slop rules --json` and the project conventions ([Project conventions win](SKILL.md#project-conventions-win)).
3. With the stand: [Before the first edit](stand.md#before-the-first-edit), steps 1-4 of stand.md. They edit no file of the repository, and their snapshot comes before step 4 edits the first one.
4. `stop-slop check --fix --json`: the program applies its safe fixes and prints what is left.
5. Fix every `error`, as its `message` says and its rule's `agent` text advises. Fix a `candidate` when its `agent` hint says it is a real problem; keep the others for step 7. Fix a `parse-error` when the branch broke the syntax, otherwise report it.
   - When a fix needs something the repository lacks (a logger, a config service), make the smallest change that meets the rule and say so in the report.
   - Moving or renaming a file or an exported name reaches other files: update every import and use of it, and note those files for the report. Move a file with `git mv`: the user sees it as a rename and can commit it apart.
6. Check each `agent` rule with `active: true`, as its `agent` text says, in the branch files its `files` covers (the first `check`'s `files` and the files you added). Fix what it finds.
7. `stop-slop check --fix --json` again. While an `error` or an undecided `candidate` is left, other than the two exceptions, go back to step 5. Then ignore each candidate you judged fine: `stop-slop ignore <fingerprint> --reason "<why it is fine here, in English>"`, with the `--base` of `check` when it needed one. Ignore last, because an edit in the same function changes the fingerprint. A final `check --json` is clean.
8. Run the project's formatter on the changed files, if the project has one (`prettier`, `biome` or `dprint` in `package.json` or a config file), through the package manager its lockfile names (npm without one). Then `check --json` again: it stays clean.
9. Run the `typecheck` and `lint` scripts of `package.json` the same way, if they exist; without a `typecheck` script, a `build` script that runs `tsc` is the typecheck. When `node_modules` is missing, install the dependencies without writing a lockfile (`npm install --no-package-lock`), and remove `node_modules` and the build output once the scripts ran. Fix what your edits broke, and note the result. If you fixed a finding of the `performance` or `branching` group, run the `test` script too: a faster rewrite or a reshaped branch can change what the code does. With no `test` script, the report says these fixes went untested. When a test fails, revert the fixes of those groups that the failing tests reach (all of them when you cannot tell which) and run `test` again. If it passes now, those fixes stay reverted and their findings stay open: do not ignore them, report each one with the test that failed. `stop-slop hook`, which the user may have set up to check each file you edit, reports a reverted `error` again: it stays open all the same. If it still fails, the failure is not theirs: put them back and note it.
10. With the stand: [Measure](stand.md#measure), steps 5-7 of stand.md, on the code as steps 4-9 left it.
11. With the stand, `stop-slop-stand clean` ([Clean](stand.md#clean)) first: it is the last command of the task. Leave the commit to the user. Report:
    - the base and why the program picked it;
    - what you fixed in the user's own lines, by rule, and plainly each fix that undoes a hunk of the user's own;
    - cleanup: what changed in old code (the `inHunk: false` findings of step 2), by file and line, so the user can commit it apart (`git add -p` where a file holds both kinds);
    - files touched by moved or renamed files and exported names, with the old findings they brought into `check`, and the state of the index: `git mv` staged the renames, other edits are unstaged and new files untracked, so the user can split the commit;
    - every ignored finding with its reason, and every rule a convention disabled;
    - the formatter, `typecheck`, `lint` and `test` results, and each reverted `performance` or `branching` fix with its finding and the failing test;
    - with the stand, its part ([stand.md](stand.md#report)).
