# Send the review

Post the draft of PR `<n>` as one review with `event=COMMENT`, after the user said to send it. The run is done when the review is on GitHub and the user has its link, or when you have told the user why nothing was sent.

1. Read `<common>/info/stop-slop/reviews/pr-<n>.md`. Every block left is a comment: its heading `## <k>. <path>:<line>` (with ` (outside the diff)` after it on some) gives the file and line, and its text runs from the line under `Rule:` to its markers, as the user left it, without the blank lines at its edges. When a block lost its heading, `Rule:` line or markers, show it to the user and send nothing until they fix it.
2. `gh pr view <n> --json headRefOid,baseRefName`. If `headRefOid` differs from the draft's `Head:`, the PR changed after the check: tell the user, offer to review it again, and send nothing.
3. Skip a block when a comment on the PR already holds each of its markers on the same file (and, for an `agent` rule's marker, on the same line). Read the inline comments (`path`, `line`, `body`) with `gh api --paginate repos/{owner}/{repo}/pulls/<n>/comments`, and the review bodies with `gh api --paginate repos/{owner}/{repo}/pulls/<n>/reviews`: there the nearest `` `path:line` `` line before a marker names its file. When every block is skipped, tell the user there is nothing new to send.
4. Put a block inline when its line is an added line (`+`) of `git diff origin/<baseRefName>...refs/stop-slop/pr-<n> -- <path>`, and into the body otherwise: the diff decides, whatever the heading says. Write the review to `<common>/info/stop-slop/reviews/pr-<n>.json`:
   - `commit_id`: the draft's `Head:`; `event`: `COMMENT`;
   - `comments`: one `{ "path", "line", "side": "RIGHT", "body" }` per inline block, where `body` is the text, a blank line and the markers; `[]` when no block is inline;
   - `body`: per block, `` `path:line` `` on its own line, a blank line, the text, a blank line and the markers, with a blank line between blocks; empty when every block is inline.
5. `gh api repos/{owner}/{repo}/pulls/<n>/reviews --method POST --input <the json>`. If GitHub still rejects it, show the user its error and the JSON's path.
6. Give the user the review's `html_url` from the response, and how many comments went inline and how many into the body.
