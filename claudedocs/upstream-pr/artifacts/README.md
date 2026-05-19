# `claudedocs/upstream-pr/artifacts/` — how to use these files

This directory contains **submission-ready text artefacts** for the
upstream pull requests our fork (`ComBba/memex`) intends to send to
`sgwannabe/memex`.

## Contents

| File | Purpose |
|---|---|
| `pr-descriptions.md` | One section per candidate PR (A–D), each with: title, GitHub-flavoured PR body, commit message, suggested labels, and reviewer profile. |
| `README.md` | (This file.) Submission workflow. |

## Status of each PR

| ID | Topic | Ship-ready? |
|---|---|---|
| A | Mix & Match modal: self-contained picker | Decided by `claudedocs/upstream-pr/candidates/pr-a/` + `reviews/pr-a/` |
| B | KF-01 path sandbox | Decided by `claudedocs/upstream-pr/candidates/pr-b/` + `reviews/pr-b/` |
| C | Defensive SVG stroke primitives | Decided by `claudedocs/upstream-pr/candidates/pr-c/` + `reviews/pr-c/` |
| D | CLI: ensure target collection | Decided by `claudedocs/upstream-pr/candidates/pr-d/` + `reviews/pr-d/` |

Do **not** open a PR for a candidate unless the corresponding
`candidates/pr-X/` + `reviews/pr-X/` agents have signed off. Drafts
exist for all four so we have them in inventory regardless of which
ones are eventually shipped.

## Submission workflow

### 0. Prerequisites

```bash
# Confirm remotes
git remote -v
# Expect:
#   origin    https://github.com/ComBba/memex.git
#   upstream  https://github.com/sgwannabe/memex.git

# Fetch latest upstream
git fetch upstream

# Confirm gh is authenticated for the upstream repo
gh auth status
gh repo view sgwannabe/memex >/dev/null
```

### 1. Create a clean topic branch off `upstream/main`

One branch per PR. Suggested names:

| PR | Branch name |
|---|---|
| A | `backport/mix-modal-self-contained-picker` |
| B | `feature/sec-path-sandbox` |
| C | `fix/svg-stroke-defensive` |
| D | `fix/cli-ensure-target-collection` |

```bash
git checkout -b backport/mix-modal-self-contained-picker upstream/main
```

### 2. Re-implement the change against `upstream/main`

These PRs are **not** cherry-picks from the fork (the source commits
depend on fork-only modules such as `codex_parser.rs`, `lens.rs`, and
fork-only HTML/CSS). For each PR, work from:

- The source commit referenced in `pr-descriptions.md` (Co-authorship
  section).
- The agent output under `claudedocs/upstream-pr/candidates/pr-X/` and
  `claudedocs/upstream-pr/reviews/pr-X/` once those exist.

Verify locally:

```bash
# Backend PRs (B, D)
cd src-tauri && cargo test --release && cargo clippy -- -D warnings

# Frontend PRs (A, C)
npm run tauri build
# then manually exercise the change in the built .app
```

### 3. Commit with the prepared message

Each PR section in `pr-descriptions.md` includes a ready commit
message. **Use `-s` for sign-off** and pipe the body via HEREDOC so
newlines are preserved:

```bash
git add <changed files>
git commit -s -m "$(cat <<'EOF'
fix(ui): make Mix & Match modal self-contained (search + add inside dialog)

The Mix & Match <dialog> is opened with showModal()...
(full body from pr-descriptions.md PR-A "Commit message" section)

Co-Authored-By: Claude Opus 4.7 (1M context) <noreply@anthropic.com>
EOF
)"
```

> Replace `<contributor>` in `Signed-off-by` with your real name/email
> via `git commit -s` (it pulls from `user.name` / `user.email`).
>
> If the upstream maintainer prefers no AI co-author trailer, drop the
> `Co-Authored-By:` line before committing.

### 4. Push to the fork remote

```bash
git push -u origin backport/mix-modal-self-contained-picker
```

### 5. Open the PR with `--body-file`

Each PR's body is plain GitHub-flavoured Markdown inside a fenced
` ```markdown ` block in `pr-descriptions.md`. **Extract the body
between the opening and closing fences only** — do *not* include the
fences themselves or any surrounding prose.

The reliable workflow:

```bash
# 1. Open pr-descriptions.md, locate the "PR A — ..." section.
# 2. Copy the contents BETWEEN the two ``` fences of the "### Body"
#    code block into a scratch file:
$EDITOR /tmp/pr-a-body.md
# (paste, save, quit)

# 3. Create the PR:
gh pr create \
  --repo sgwannabe/memex \
  --base main \
  --head ComBba:backport/mix-modal-self-contained-picker \
  --title "fix(ui): make Mix & Match modal self-contained (search + add inside dialog)" \
  --body-file /tmp/pr-a-body.md
```

> Why not pipe the file directly? Because `pr-descriptions.md` wraps
> each PR body inside a fenced block to keep it readable. Extracting
> the inner content avoids leaking ` ``` ` markers into the GitHub
> rendering.

If you want to fully automate the extraction, this awk one-liner picks
the body of a named section (replace `PR A` with the section header):

```bash
awk '
  /^## PR A /        { in_sec = 1; next }
  /^## PR [B-Z] /    { in_sec = 0 }
  in_sec && /^### Body/  { in_body = 1; next }
  in_sec && in_body && /^```markdown$/ { capture = 1; next }
  in_sec && in_body && /^```$/         { capture = 0; in_body = 0 }
  capture { print }
' claudedocs/upstream-pr/artifacts/pr-descriptions.md > /tmp/pr-a-body.md
```

Sanity-check the extracted body before submitting:

```bash
gh pr create --dry-run \
  --repo sgwannabe/memex \
  --base main \
  --head ComBba:backport/mix-modal-self-contained-picker \
  --title "..." \
  --body-file /tmp/pr-a-body.md
```

### 6. Add labels and request review

Each PR section lists `Suggested labels` and `Reviewer suggestions`.
Add the ones that exist in the upstream repo:

```bash
# List available labels in upstream
gh label list --repo sgwannabe/memex

# Apply (omit labels that don't exist upstream)
gh pr edit <pr-number> --repo sgwannabe/memex \
  --add-label bug --add-label ui --add-label frontend
```

Reviewer suggestion is informational — the maintainer auto-assigns.

### 7. After opening

- Update each PR's "Test plan" section once the corresponding
  `test-plan.md` lands under `claudedocs/upstream-pr/candidates/pr-X/`.
  Edit the PR body inline via `gh pr edit <n> --body-file ...`.
- Replace the screenshot/recording placeholders by dragging files
  into the PR comment box (GitHub uploads them and inserts the
  Markdown).
- Respond to maintainer feedback in new commits (no force-push, no
  squash — preserve commit history per fork policy).

## What to do if a PR is rejected

- Capture the maintainer's reasoning in
  `claudedocs/upstream-pr/reviews/pr-X/maintainer-feedback.md`.
- Update `claudedocs/UPSTREAM_PR_PLAN.md` §2 with the rejection note
  so future sessions don't re-attempt without addressing the
  feedback.
- The fork keeps the change locally regardless — these PRs are best-
  effort upstream offers, not blocking dependencies for the
  hackathon.

## Editing the descriptions

If a candidate's agent output reveals the PR scope needs to change
(e.g. additional files, different rationale), edit `pr-descriptions.md`
in place rather than maintaining drift between files. The artefact is
the source of truth for the submission text.
