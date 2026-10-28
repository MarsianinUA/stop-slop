# Review a PR

Draft review comments for a colleague's PR `<n>`. The run ends when the draft file is written and the user has its path; sending is a separate run ([send.md](send.md)), after the user says so. The working tree stays as it is.

1. `stop-slop check --pr <n> --json`. It fetches the PR head into `refs/stop-slop/pr-<n>` and its base into `origin/<pr.base>`, where `pr` is the field of its output. When it exits with 2 and `next` holds this same command, the PR moved during the fetch: run it once more rather than stop.
2. Read `stop-slop rules --json`, and the project conventions ([Project conventions win](SKILL.md#project-conventions-win)) at the PR revision: list them with `git ls-tree -r --name-only <pr.head>`, read them with `git show <pr.head>:<path>`.
3. Judge every finding against the PR's code (`git show <pr.head>:<file>`) and its diff (`git diff origin/<pr.base>...<pr.head> -- <file>`). Drop:
   - false positives: a `candidate` its `agent` hint clears (where a hint says to ignore a finding, a review drops it), or an `error` the code proves wrong;
   - what a convention allows ([Project conventions win](SKILL.md#project-conventions-win));
   - what is not the author's: an `inHunk: false` finding whose function the PR left alone, unless the PR made it worse. The function is the innermost one around the finding (for a parameter, the function that declares it), or its top-level declaration outside any function; the PR touched it when the diff changes a line of its code, not only its comments.

   When most of the codebase follows a habit a rule flags and no convention says so (every handler names its context `c`), keep the finding and point it out to the user. A `parse-error` goes to the user, not into a comment.
4. Check each `agent` rule with `active: true`, as its `agent` text says, in what the PR adds or changes in the files its `files` covers.
5. Write a comment for every finding left (see Comment). The findings of one rule in one function make one comment, on the first line, naming the other lines.
6. Write the draft to `<common>/info/stop-slop/reviews/pr-<n>.md` (create the folder if needed). Give the user its path, how many findings you dropped and why (by reason), and the comments to look at twice: habits of the codebase and close calls. Say that deleting a block drops that comment and that "send" posts the rest.

## Comment

- In English: short (two or three sentences, and a code sample when it helps), plain words, gentle, written for someone who does not know the whole codebase.
- Says what to change and why. The rule's `message` gives the why; the rule id stays in the draft's `Rule:` line.
- On old code (`inHunk: false`), says the line is older than the PR and asks for the change while the function is open anyway.
- When the project's conventions have the rule, links that section at the PR head: `[CONVENTIONS.md → Config Access](https://github.com/<owner>/<repo>/blob/<pr.head>/<path>#<anchor>)`.
- Goes through the `unslop` skill; where it pulls against the points above, they win.

## Draft

The draft opens with `# PR <n>: <pr.title>` and `Head: <pr.head>`. The text under the `Rule:` line is the comment exactly as it will be posted. `(outside the diff)` marks an `inHunk: false` finding: it goes into the review's body, because inline comments sit on the lines the PR changes. The last lines of a block are its markers, one per finding: `<!-- stop-slop:<fingerprint> -->`, or `<!-- stop-slop:<rule> -->` for an `agent` rule. They stop a second run from posting the comment again.

```md
# PR 123: Cache market prices
Head: 9f2c4e1d0b7a6c5e4f3a2b1c0d9e8f7a6b5c4d3e

## 1. src/modules/markets/markets.service.ts:42
Rule: no-process-env (CONVENTIONS.md → Config Access)

Could we read this through ConfigService instead of process.env? Runtime code gets its config from there, and process.env stays in `*.config.ts` files. See [CONVENTIONS.md → Config Access](https://github.com/acme/shop/blob/9f2c4e1d0b7a6c5e4f3a2b1c0d9e8f7a6b5c4d3e/CONVENTIONS.md#config-access).

<!-- stop-slop:no-process-env:3f9a1c -->

## 2. src/modules/markets/markets.mapper.ts:8 (outside the diff)
Rule: no-boolean-param

...
```
