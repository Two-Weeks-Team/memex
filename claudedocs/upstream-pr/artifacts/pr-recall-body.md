## Summary

Two small, independent UX improvements that surfaced while debugging the recall banner and the stack-card badges. Both fixes are tightly scoped to existing code paths — no new dependencies, no schema changes, no public API changes.

1. **`tail_recent_errors`** drops a body-text regex that was producing false-positive recall banners for benign shell stderr, user prose, and test output. It now relies solely on Claude's structured tool-result `is_error: true` signal, plus a small shell-stderr blocklist.
2. The red **"errors" badge** on stack cards gains a `title=` tooltip clarifying that the badge reflects a session-internal failed tool call (not a Memex runtime error).

## Problem / Motivation

### 1. Recall banner surfaced non-actionable noise

`tail_recent_errors` polls recent JSONL session files (last 60s by default) for the most recent error in each, then renders a banner like:

> "memex just hit: 'Exit code 3 jq: error: syntax error...'"

The intent is "past-you ran into something like this — here's a hint", which is a useful signal **when the error is a recurring runtime failure**. But the previous implementation matched any line containing `"error:"`, `"traceback"`, or `"panic"` anywhere in the last 6 turns' body text. That caught:

- **Shell stderr passthrough** from one-off CLI commands invoked by tools (`jq: error: syntax error`, `rg: no match`, `cat: file not found`). These are typos/quoting glitches, not bugs to learn from.
- **User prose** in messages like `"I'm getting an error: …"` — that's the user asking a question, not the runtime reporting one.
- **Test output** that mentions the literal word `panic` as a function/test name without an actual panic occurring.
- **Parse-error literals** appearing inside JSON examples being copy-pasted into the conversation.

None of those are actionable recall hints. They added noise without adding signal, and several users reported finding the banner more confusing than helpful.

### 2. "errors" badge label was ambiguous

Stack cards render `'<span class="badge err">errors</span>'` when `payload.has_errors === true`. The underlying signal is meaningful — at least one `is_error: true` tool result in the session, surfaced for "this past session ran into snags" navigation. But the bare label "errors" with no tooltip can read as "Memex itself errored" rather than "this session contained a failed tool call". Users repeatedly asked what the badge meant.

## Solution

### 1. `src-tauri/src/commands.rs::tail_recent_errors`

- **Remove** the body-text fallback regex (`error:` / `traceback` / `panic` line scan).
- **Keep** the structured-tool-result path as the sole signal.
- **Add** a small shell-stderr blocklist applied to the structured error head:
  - `starts_with("exit code")`
  - contains `"syntax error"`
  - contains `"command not found"`
  - contains `"no such file or directory"`
  - contains `"unbound variable"`
  - contains `"parse error"`
- When the head matches the blocklist, `continue` to the next older turn rather than returning empty — so a real error a few turns back still surfaces.

The cache machinery (`TAIL_CACHE`, negative caching, mtime-keyed invalidation) is untouched.

### 2. `src/main.js::renderStack`

Added a `title=` attribute to the errors badge:

> "This session contains at least one failed tool call (Bash exit non-zero / Edit denied / Read missing file / etc.). Click the card → Replay to see which turn."

The badge is otherwise unchanged. The tooltip is browser-native — no JS handlers, no a11y regressions, works on keyboard focus via `title`.

## Trade-offs / Alternatives considered

- **Make the body-text fallback opt-in via a flag.** Rejected — the configuration cost (state, UI surface, docs) outweighs the value of a fallback most users disabled anyway.
- **Use a richer popover instead of `title`.** Rejected for scope — `title` is universally supported, requires zero extra code, and matches the badge's lightweight role. A popover could be a separate follow-up if the badge gains more affordances.
- **Whitelist shell errors (only let real runtime errors through) instead of blocklist.** Rejected — the universe of "real runtime errors" is too broad to enumerate; the noise is the long tail. A blocklist of ~6 patterns covers the bulk of what users actually saw.
- **Drop the structured-error head limit (currently 800 chars).** Out of scope — the limit was correct before and remains so.

## Test plan

Existing `cargo test --lib --tests` passes (14 parser integration tests, the only existing tests touching this code path). No new tests in this PR — the recall filter is a localized refinement of an existing function, and the JS change is a single attribute addition.

Manual smoke (single-window Tauri):

- [ ] `npm run tauri dev` boots without console errors.
- [ ] Run a CLI command in a session that produces shell stderr (e.g., `jq` with a syntax error). Verify the recall banner does **not** appear for that session within 60s.
- [ ] Trigger a real structured tool error (e.g., `Edit` to a path you don't own). Verify the banner **does** appear.
- [ ] Hover the red "errors" badge on a stack card. Verify the tooltip appears and reads as expected.

## Backwards compatibility

- **No public API change.** `tail_recent_errors` keeps its `(path, since_seconds) → Vec<RecentError>` signature; `RecentError` struct unchanged.
- **No schema or storage change.** No Qdrant payload field added/removed/renamed.
- **No new dependencies.** Cargo.toml untouched.
- **Frontend.** The badge tooltip is a single attribute addition — no DOM structure change, no event handler, no a11y regression.
- **Behavior change.** Banners that previously fired off body-text matches will no longer fire. This is the desired behavior change, not a regression — users were asking for this filter.

## Provenance

These changes were sliced out of fork commit [`84db1fc`](https://github.com/ComBba/memex/commit/84db1fc6fa21e8333836b36c2bb7875b8332510b) in `ComBba/memex`. The original commit additionally enabled `tauri = { features = [..., "devtools"] }` for in-fork debugging; **that piece is intentionally omitted from this PR** because shipping the WebKit Web Inspector in release builds widens the attack surface (IPC introspection from any opened DOM) and is not appropriate for upstream. The two improvements in this PR are functionally independent of the devtools change.
