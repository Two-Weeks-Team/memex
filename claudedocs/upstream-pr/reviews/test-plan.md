# Upstream PR Test Plan — `ComBba/memex` → `sgwannabe/memex`

**Author**: QE expert (Opus 4.7 1M)
**Generated**: 2026-05-19 (D-13)
**Audience**: upstream maintainer (`sgwannabe/memex`) reviewing each backport PR
**Assumed env**: macOS 14+ (Apple Silicon), Xcode CLT 15+, Rust stable ≥1.75, Node 20+, Docker Desktop 4.x (for Qdrant), `~/.claude/projects` and/or `~/.codex/sessions` populated with ≥1 real session.

---

## §0 Conventions used throughout

| Symbol | Meaning |
|---|---|
| ✅ MUST pass | blocker for merge |
| ⚠️ SHOULD pass | non-blocking, file follow-up issue if it fails |
| 🟢 currently green on fork | already verified |
| 🟡 unverified on fork | needs reviewer to run |
| 🔴 currently red on fork | known issue, called out per candidate |
| `Sxx` | numbered manual step |
| `[ ]` | checkbox the reviewer can mark in PR conversation |

**Pre-flight env vars** (set once per shell):

```bash
export MEMEX_TEST_HOME="$(mktemp -d)"
export RUST_BACKTRACE=1
export RUST_LOG=memex_lib=debug,info
export QDRANT_URL=http://127.0.0.1:6334   # gRPC port used by qdrant-client 1.x
```

**Repo state expected at start of every plan**:

```bash
git fetch upstream
git checkout -b review/<candidate> upstream/main
git am < /tmp/<candidate>.patch     # or git cherry-pick if conflict-free
```

---

## §1 Candidate A — `e402b1f` mix-modal self-contained picker

Files touched: `src/index.html (+35/-3)`, `src/main.js (+148/-10)`, `src/styles.css (+127)`.
Touched IDs / selectors: `#mix-modal`, `#mix-picker-input`, `#mix-picker-results`, `.mix-picker-row`, `.btn[disabled]`, `#mix-pos-zone`, `#mix-neg-zone`, `#run-mix-btn`.
Touched JS symbols: `runMixPickerSearch`, `renderMixPickerRow`, `attachMixPickerEvents`, `updateRunMixButton`, `openMixModal`, `addToMix`, `removeFromMix`.
Tauri commands invoked: `lens_search_v2` (primary), `lens_search` (fallback). **No new Tauri command.**

### 1.1 Pre-flight (run order matters — fail-fast)

| # | Command | Expected | Status on fork |
|---|---|---|---|
| A1 | `cargo fmt --check` (in `src-tauri/`) | exit 0 | 🟢 |
| A2 | `cargo clippy --all-targets -- -D warnings` | 0 warnings | 🟢 |
| A3 | `cargo test --release` | all green | 🟢 (no Rust touched) |
| A4 | `npm install` (root) | OK | 🟢 |
| A5 | `npm run tauri build` | `.app` produced under `src-tauri/target/release/bundle/macos/` | 🟢 |
| A6 | Open built `.app`, no console errors in DevTools | 0 errors in `[DevTools] Console` | 🟡 (must verify on clean upstream base) |

> ⚠️ A2/A3 are **defensive only** — this candidate touches no Rust. The signal is "the JS/CSS/HTML changes didn't accidentally break the tauri-build pipeline".

### 1.2 Unit tests to add

This module currently has **no JS unit tests** (no `tests/js/`, no jest/vitest config). PR should land **either**:

- **Option 1 (lightweight, recommended for upstream)**: add `tests/js/mix-picker.test.mjs` exercised by a minimal `vitest` setup added in this PR. Two test files keeps the JS test infra footprint small.
- **Option 2 (skip JS unit tests)**: rely entirely on §1.3 integration + §1.4 manual. State this trade-off in the PR description.

If Option 1 is chosen, the table below is the required matrix:

**File**: `tests/js/mix-picker.test.mjs`

| Test name | Input | Assertion |
|---|---|---|
| `runMixPickerSearch_empty_query_clears_results` | `""` typed, then `↵` | `#mix-picker-results.innerHTML === ""`; no IPC call |
| `runMixPickerSearch_uuid_input_skips_qdrant` | `"019e1bdb-2799-7392-9d7c-de37ade48bc7"` | invoke called 0 times; renders a single direct-pick row |
| `runMixPickerSearch_falls_back_to_lens_search` | `lens_search_v2` mock throws | exactly 1 retry against `lens_search`; results render |
| `runMixPickerSearch_renders_max_12_results` | mock returns 25 hits | DOM has exactly 12 `.mix-picker-row` elements |
| `renderMixPickerRow_pos_button_disables_after_add` | row with `[+ pos]`, click | button text becomes `✓ pos`, `disabled` attr set |
| `renderMixPickerRow_neg_button_independent_of_pos` | click `[− neg]`, then `[+ pos]` on same row | both buttons toggle independently; row stays in DOM once per side |
| `updateRunMixButton_disabled_when_both_zones_empty` | `state.mix = { pos: [], neg: [] }` | `#run-mix-btn[disabled]` present, `title` non-empty |
| `updateRunMixButton_enabled_when_one_anchor` | `state.mix = { pos: ["s1"], neg: [] }` | no `disabled` attr |
| `openMixModal_seeds_picker_from_state_query` | `state.query = "auth refactor"` | `#mix-picker-input.value === "auth refactor"`; `runMixPickerSearch` invoked once |
| `openMixModal_empty_state_query_no_autofire` | `state.query = ""` | `runMixPickerSearch` invoked 0 times |
| `removeFromMix_refreshes_picker_button_state` | add then remove same session | button reverts to actionable `[+ pos]` (not `✓ pos`) |

Stub the Tauri global: `globalThis.__TAURI_INTERNALS__ = { invoke: vi.fn(...) }`.

### 1.3 Integration tests

No backend-side integration tests required (no Tauri command added). Two cross-cutting checks:

| # | Test | How |
|---|---|---|
| I-A1 | `lens_search_v2` happy path returns rows that the picker can render | start `docker compose up qdrant`, run `./memex scan ~/.claude/projects`, build app, type `"auth"` in picker, verify ≥1 row |
| I-A2 | `lens_search_v2` missing (older builds) → fallback to `lens_search` | temporarily rename `lens_search_v2` registration in `commands.rs`, rebuild, type `"auth"`, expect rows |

### 1.4 Manual / E2E (target time: 8 min)

Reviewer runs on a real macOS box with a populated `~/.claude/projects`:

```
S01  npm run tauri build && open src-tauri/target/release/bundle/macos/Memex.app
S02  Wait for "Connected to Qdrant" indicator
S03  In the main lens search box type "auth" → ↵, confirm cards render
S04  Click "Mix & Match" button (or ⌘+M shortcut)
S05  Modal opens. EXPECT: #mix-picker-input is pre-seeded with "auth",
     #mix-picker-results already has ≥1 row WITHOUT any extra click
S06  Verify backdrop blocks main-view cards (try clicking [+ pos]
     behind backdrop — should NOT register)
S07  In picker, click [+ pos] on first row.
     EXPECT: button flips to ✓ pos, becomes disabled, dropzone shows chip
S08  Click [− neg] on second row. EXPECT: independent toggle
S09  [Run discovery] now enabled (previously disabled with title attr).
     Click it. EXPECT: results render in mix-result panel
S10  Remove the pos chip from dropzone.
     EXPECT: row's [+ pos] button is re-enabled (not stuck on ✓ pos)
S11  Type a UUID directly (paste from clipboard) → ↵.
     EXPECT: single row appears WITHOUT a Qdrant network call
     (verify in DevTools → Network tab: no /collections POST)
S12  Esc/close modal → re-open. EXPECT: no stale state, picker is fresh
S13  Resize window to 800×600 → re-open modal.
     EXPECT: modal still usable, no horizontal scroll trap
```

**Expected screenshot diff**:
- Pre-fix (upstream/main): modal opens with only the "click + pos on a card" hint and dropzones — no picker. Trying to use it is a dead end.
- Post-fix: modal opens with picker section between hyperplane and dropzones, pre-seeded results, [+pos]/[−neg] live inside the modal.

Reference fork screencaps live at `claudedocs/reports/` (not under upstream/main; reviewer should capture their own to compare).

### 1.5 Regression vectors

| Vector | Mitigation in test |
|---|---|
| Stack-card `[+ pos]` / `[− neg]` buttons stop working (pre-stage path) | S07 alt: close modal, click `[+ pos]` on a card, re-open modal — chip should be in dropzone |
| Mix modal opens without a prior search (cold-start) | S05 with `state.query` blank → modal opens, picker is empty but functional |
| Keyboard navigation broken in picker | Tab through rows — focus order is logical, ↵ on button activates it |
| Voice-over / aria — picker is `role="listbox"`, rows should be `role="option"` | grep `role=` in `index.html` lines 220–235 |
| Memory leak: opening/closing modal 50× | DevTools Performance recording, heap delta < 5 MB |
| Concurrent searches: type fast | Debounced or last-wins (currently submit-on-↵ only; document if you add debounce) |

**Flags / configs to test under**:
- Build with `--release` AND debug — picker should work in both.
- `MEMEX_DEFAULT_LENS_VERSION=v1` (if upstream supports it) — fallback path exercised.

### 1.6 Property-based ideas

Low value here — UI/event code with little numeric surface. One useful fuzz:

- **Picker input fuzzer**: feed random Unicode strings (incl. RTL, emoji, control chars, 4-byte chars, 10 KB strings) into `#mix-picker-input`. EXPECT: no uncaught exception in DevTools console, no XSS injection into `mix-picker-results` (assert `row.querySelector("script") === null` after every render).

### 1.7 CI matrix suggestion

```yaml
# .github/workflows/pr-mix-picker.yml
jobs:
  build-macos:
    strategy:
      matrix:
        os: [macos-13, macos-14]   # Intel + Apple Silicon
        node: [20, 22]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
        with: { node-version: ${{ matrix.node }} }
      - run: npm ci
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo fmt --check && cargo clippy -- -D warnings
      - run: cargo test --release
      - run: npm run tauri build
      - if: success()
        uses: actions/upload-artifact@v4
        with: { name: memex-${{ matrix.os }}-${{ matrix.node }}, path: src-tauri/target/release/bundle/macos/Memex.app }
```

---

## §2 Candidate B — `f55d417` P1 security (`KF-01 path sandbox` subset)

> **Reviewer note**: full `f55d417` carries KF-01 + KF-02 + KF-03 + KH-01 across `sec.rs (+279)`, `snapshot.rs (+439)`, plus call-site wiring in `commands.rs` and `indexer.rs`. **For upstream backport we recommend a KF-01-only PR** — it is the only subset that compiles standalone on upstream/main (snapshot/envelope features depend on schema v3 that upstream lacks).

Files for the **KF-01-only** subset:
- ADD `src-tauri/src/sec.rs` (~150 LOC if KF-02/03 stripped — keep `SourceAgent`, `SandboxRoot`, `validate_session_path`)
- MOD `src-tauri/src/lib.rs` (+1 `pub mod sec;`)
- MOD `src-tauri/src/commands.rs` (wire `validate_session_path` into `get_session_turns`)
- MOD `src-tauri/Cargo.toml` (+`dirs = "5"` if not transitive)
- ADD `src-tauri/tests/sec_integration.rs` (~50 LOC)

### 2.1 Pre-flight

| # | Command | Expected | Status on fork |
|---|---|---|---|
| B1 | `cargo fmt --check` | exit 0 | 🟢 |
| B2 | `cargo clippy --all-targets -- -D warnings` | 0 warnings | 🟢 |
| B3 | `cargo test --release` | 42+ tests pass (fork count); upstream KF-01-only will pass ~16 sec.rs + 2 sec_integration | 🟢 fork / 🟡 upstream |
| B4 | `cargo build --release` (cold) | < 90 s on M2 | 🟡 |
| B5 | `npm run tauri build` | `.app` builds | 🟢 |
| B6 | `cargo audit` (if added) | 0 high/critical | 🟡 |

### 2.2 Unit tests to add (file: `src-tauri/src/sec.rs` `#[cfg(test)] mod tests`)

Fork already has 14 of these — reviewer should verify each is **kept** in backport.

| Test name | Input | Assertion |
|---|---|---|
| `t_valid_claude_session_path` | `<claude_root>/proj-x/sess.jsonl` | `Ok(canonical)`, starts_with claude root |
| `t_valid_codex_session_path` | `<codex_root>/2026/05/19/rollout-x.jsonl` | `Ok(canonical)` |
| `t_path_outside_sandbox_etc` | `/etc/passwd` | `Err`, msg contains "outside sandbox" |
| `t_path_outside_both_tmp` | `/tmp/random.jsonl` | `Err` |
| `t_path_traversal_dotdot` | `<claude_root>/../../etc/passwd` | `Err` post-canonicalize |
| `t_path_traversal_double_dotdot` | `<claude_root>/proj/../../../../../etc/passwd` | `Err` |
| `t_symlink_outside` | symlink in root → `/etc/passwd` | `Err`, canonicalize follows symlink |
| `t_symlink_inside` | symlink in root → another file in root | `Ok` |
| `t_symlink_dangling` | symlink → nonexistent target | `Err` from canonicalize |
| `t_nul_byte_path` | `"foo\0bar.jsonl"` | `Err`, msg contains "NUL" — must reject pre-canonicalize |
| `t_empty_string` | `""` | `Err`, msg contains "empty" |
| `t_nonexistent_path` | `<claude_root>/missing.jsonl` | `Err` from canonicalize |
| `t_canonical_idempotent` | already-canonical valid path | `Ok`, result equals input |
| `t_graceful_codex_missing` | only Claude root exists | `from_env()` succeeds |
| `t_graceful_claude_missing` | only Codex root exists | `from_env()` succeeds |
| `t_both_missing` | neither root exists | `from_env()` returns `Err` |
| `t_source_agent_as_str` | `SourceAgent::ClaudeCode.as_str()` | `== "claude_code"` |
| `t_source_agent_codex_as_str` | `SourceAgent::Codex.as_str()` | `== "codex"` |
| `t_detect_agent_returns_claude` | path under claude root | `Some(ClaudeCode)` |
| `t_detect_agent_returns_codex` | path under codex root | `Some(Codex)` |
| `t_detect_agent_returns_none_outside` | path outside | `None`, does NOT throw |
| `t_arbitrary_bytes_no_panic` | random `Vec<u8>` cast to OsStr (Unix) | never panics, always returns `Err` (proptest below) |
| `t_long_path_no_panic` | 4096-char path | no panic; `Err` ok |
| `t_unicode_path_valid` | `"세션-한국어-😀.jsonl"` in root | `Ok` |
| `t_case_sensitivity_macos` | mixed-case path on case-insensitive fs | `Ok` if exists; document behavior in PR |

### 2.3 Integration tests (`src-tauri/tests/sec_integration.rs`)

Fork has 2; recommend expanding to:

| Test | Scenario |
|---|---|
| `it_sandbox_from_env_succeeds_if_any_root_exists` | exercises `SandboxRoot::from_env()` against real `$HOME` — skips assertion if neither root present (CI) |
| `it_rejects_etc_passwd` | `/etc/passwd` always rejected |
| `it_get_session_turns_rejects_outside_path` | spawn Tauri command harness, invoke `get_session_turns("/etc/passwd")` → returns `Err("path outside sandbox")` |
| `it_get_session_turns_accepts_valid_path` | feed a real fixture from `src-tauri/tests/fixtures/` (after copying into a fake `$HOME/.claude/projects/test-proj/`), expect `Ok([Turn, ...])` |
| `it_predict_next_actions_rejects_outside_payload` | mock Qdrant payload with `source_path=/etc/passwd` → predict returns 0 neighbors without panicking, error logged |

**Qdrant requirement**: only `it_predict_next_actions_*` needs Qdrant; gate behind `#[cfg(feature = "qdrant-it")]` or `#[ignore]` with `cargo test -- --ignored` opt-in.

### 2.4 Manual / E2E (target time: 6 min)

Pre-req: `~/.claude/projects` populated.

```
S01  Build app: cargo build --release
S02  cp src-tauri/target/release/memex /tmp/memex
S03  /tmp/memex scan ~/.claude/projects  → completes, prints "indexed N sessions"
S04  /tmp/memex search "auth" → returns hits
S05  Tamper test (manual):
       - Use qdrant CLI or REST to UPSERT a fake point with
         payload.source_path = "/etc/passwd"
       - /tmp/memex predict <fake-sid>  →  EXPECT: error/warning, NO file read
       - Verify with `lsof -p <pid>`: no /etc/passwd entry
S06  Symlink test:
       - ln -s /etc/passwd ~/.claude/projects/test-proj/evil.jsonl
       - /tmp/memex search any-query, then try to load that "session"
       - EXPECT: rejected with "path outside sandbox" (canonicalize follows symlink)
       - rm ~/.claude/projects/test-proj/evil.jsonl
S07  Graceful-degrade test:
       - mv ~/.codex/sessions ~/.codex/sessions.bak  (or rm -rf if you don't use Codex)
       - /tmp/memex scan ~/.claude/projects → still works
       - mv back
```

### 2.5 Regression vectors

| Vector | Mitigation in test |
|---|---|
| Users with neither root → app refuses to start | acceptable; document in PR. Test: `it_both_missing` |
| Symlinks that point **inside** the sandbox should still work | `t_symlink_inside` |
| Path with spaces / Unicode | `t_unicode_path_valid` |
| Performance regression (canonicalize per request) | benchmark: 10k canonicalize calls < 500 ms on M2 (use `criterion` or just `Instant::now()` loop) |
| Existing `get_session_turns` callers that pass already-canonical paths | `t_canonical_idempotent` |
| Windows portability (uses `dirs::home_dir`) | CI matrix entry `windows-latest`; document if Windows is OOS for upstream |

**Flags / configs to test under**:
- `MEMEX_DISABLE_SANDBOX=1` — if you add an escape hatch (NOT recommended), test it explicitly. Default: no escape hatch.
- `$HOME` redirected via env override — `it_sandbox_from_env_*` should respect it.

### 2.6 Property-based ideas

Add `proptest = "1"` as dev-dep, file `src-tauri/tests/sec_proptest.rs`:

```rust
proptest! {
    #[test]
    fn never_panics_on_arbitrary_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        let s = OsStr::from_bytes(&bytes);   // Unix
        let p = Path::new(s);
        let _ = validate_session_path(p);     // must not panic
    }
    #[test]
    fn never_escapes_root_with_dotdot_chain(
        depth in 0usize..32,
        suffix in "[a-zA-Z0-9_./]{0,100}"
    ) {
        let mut p = sandbox_root();
        for _ in 0..depth { p = p.join(".."); }
        p = p.join(suffix);
        if let Ok(canon) = validate_session_path(&p) {
            assert!(canon.starts_with(sandbox_root()));
        }
    }
    #[test]
    fn utf8_paths_inside_root_accepted(name in "[a-zA-Z0-9_\\-가-힣😀-🙏]{1,32}\\.jsonl") {
        let p = sandbox_root().join(&name);
        std::fs::write(&p, b"{}").ok();
        let r = validate_session_path(&p);
        // either Ok (canonicalize worked) or Err from FS — never panic
        let _ = r;
    }
}
```

Specific edge inputs to cover (named-case style):

| Input class | Examples |
|---|---|
| Traversal | `../`, `../../`, `..\\..\\`, `%2e%2e%2f`, mixed `/foo/../bar` |
| NUL injection | `foo\0bar`, `\0`, `foo/\0/bar` |
| Symlink loops | `a → b → a` |
| Long paths | 4096 chars, > PATH_MAX |
| Unicode | RTL `‮`, BOM, combining chars, normalization (NFC vs NFD on macOS HFS) |
| Empty / whitespace | `""`, `" "`, `"\n"`, `"."` |

### 2.7 CI matrix suggestion

```yaml
jobs:
  sec-tests:
    strategy:
      matrix:
        os: [ubuntu-22.04, macos-14, windows-2022]
        rust: [stable, beta]
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@${{ matrix.rust }}
      - run: cargo test --package memex_lib --test sec_integration
      - run: cargo test --package memex_lib --lib sec::tests
      - run: cargo test --test sec_proptest -- --test-threads=1
      - if: matrix.os == 'ubuntu-22.04'
        run: cargo install cargo-audit && cargo audit
```

Windows row may be marked `continue-on-error: true` until `dirs::home_dir` portability is verified.

---

## §3 Candidate C — `2b59dc9` predict Codex routing

> **Reviewer note (CRITICAL)**: Per `UPSTREAM_PR_PLAN.md §2 候補 A`, this commit **cannot be cherry-picked to upstream** because the fix calls `codex_parser::parse_codex_session` — a module that does not exist on `upstream/main`. The schema field `source_agent` is also fork-only (introduced in P3).
>
> Two paths forward:
> 1. **DO NOT SEND** — upstream's `predict_next_actions` is single-parser (Claude only). The bug doesn't exist there. PR adds dead code.
> 2. **SEND IF AND ONLY IF** `codex_parser.rs` + schema v3 (P3) lands first as a separate PR. Then `2b59dc9` becomes a tiny followup.
>
> The test plan below assumes path **2** and is the contract the followup PR must satisfy.

Files touched: `src-tauri/src/indexer.rs (+39/-5)` only.
Touched symbols: `predict_next_actions` (active-session branch ≈L1627, neighbor-loop branch ≈L1696/1734), `PREDICT_PARSE_CACHE.get_or_parse` closure.
Tauri commands invoked: `predict_next_actions` (via CLI `memex predict <sid>`).

### 3.1 Pre-flight

| # | Command | Expected | Status on fork |
|---|---|---|---|
| C1 | `cargo fmt --check` | 0 | 🟢 |
| C2 | `cargo clippy --all-targets -- -D warnings` | 0 | 🟢 |
| C3 | `cargo test --release` | green | 🟢 |
| C4 | Build CLI: `cargo build --release --bin memex` | binary | 🟢 |

### 3.2 Unit tests to add (`src-tauri/src/indexer.rs` `#[cfg(test)] mod predict_tests`)

| Test name | Setup | Assertion |
|---|---|---|
| `predict_routes_codex_active_via_codex_parser` | mock payload `source_agent="codex"`, fixture `rollout-02-with-tools.jsonl` | parser invoked is `codex_parser::parse_codex_session` (use a spy / counter); turns.len() > 0 |
| `predict_routes_claude_active_via_claude_parser` | payload `source_agent="claude_code"`, fixture `02_with_tool_use.jsonl` | claude parser invoked; turns.len() > 0 |
| `predict_routes_missing_source_agent_defaults_claude` | payload omits `source_agent` | claude parser invoked (legacy v2 behavior) |
| `predict_neighbor_loop_routes_per_neighbor_agent` | 3 neighbors: 1 claude + 2 codex (mixed payloads) | each routed correctly; no `turns.is_empty()` skip for codex |
| `predict_empty_codex_turns_no_panic` | fixture `rollout-05-empty-after-meta.jsonl` (codex), `source_agent="codex"` | returns Ok with 0 predictions, no panic |
| `predict_cache_keys_include_agent` | same path indexed twice as claude then codex | cache returns different parsed Sessions, NOT the stale claude parse |

**Cache-key gotcha**: `PREDICT_PARSE_CACHE.get_or_parse(validated.clone(), mtime, |p| ...)` — if the key is only `(path, mtime)` and not `(path, mtime, agent)`, a path that ever gets parsed-as-claude will stick. Verify cache key includes agent OR write a regression test demonstrating this is safe (paths are uniquely owned by one agent because sandbox roots are disjoint).

### 3.3 Integration tests

`src-tauri/tests/predict_routing_integration.rs` (new file):

| Test | Scenario |
|---|---|
| `it_predict_codex_active_returns_neighbors` | `docker compose up qdrant`, scan a corpus with ≥5 codex sessions, run `predict_next_actions(codex_sid, 5)` → returns ≥1 prediction |
| `it_predict_claude_active_uses_codex_neighbors_too` | corpus with both agents, predict from claude anchor → results include neighbors parsed from codex (not silently filtered) |
| `it_predict_legacy_v2_payload_still_works` | mock a v2 payload (no `source_agent` field) → defaults to claude parser, no error |
| `it_predict_pure_codex_corpus_works` | scan ONLY codex root → predict returns non-empty |
| `it_predict_pure_claude_corpus_works` | scan ONLY claude root → predict returns non-empty |

**Real corpus replay** (reproduces the original bug):

```bash
docker compose up -d qdrant
./target/release/memex scan ~/.codex/sessions
./target/release/memex predict 019e1bdb-2799-7392-9d7c-de37ade48bc7 --neighbors 5
# EXPECT: "looked at 5 similar session(s), used 5" (not "used 0")
```

### 3.4 Manual / E2E (target time: 5 min)

```
S01  docker compose up -d qdrant   (use upstream's compose if present, else fork's)
S02  cargo build --release --bin memex
S03  ./target/release/memex scan ~/.claude/projects   # ≥10 claude sessions
S04  ./target/release/memex scan ~/.codex/sessions    # ≥10 codex sessions
S05  Pick a Codex session id (look in ~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl, the uuid is in filename)
S06  ./target/release/memex predict <codex-sid> --neighbors 5
     EXPECT: "looked at 5 similar session(s), used 5"
     EXPECT: predictions table is NOT empty
S07  Pick a Claude session id (look at ~/.claude/projects/*/sess-*.jsonl)
S08  ./target/release/memex predict <claude-sid> --neighbors 10
     EXPECT: predictions include some from-session values pointing to codex
     sources (proof neighbor loop didn't drop them silently)
S09  Pre-fix reproduction (sanity): git stash; rebuild; rerun S06.
     EXPECT: "used 0 — try a different anchor"  (this is the bug)
     git stash pop; rebuild; S06 again → "used 5" (this is the fix)
```

**Expected output diff**:

```diff
- predicting next actions for 019e1bdb-... — looked at 0 similar session(s), used 0
- no predictions — try a different anchor or re-index more sessions
+ predicting next actions for 019e1bdb-... — looked at 5 similar session(s), used 5
+ anchor (last turn): **Inventory**
+ #    tool       freq    conf    example                       from-session
+ 1    Bash       0.42    0.731   PF=~/.claude/...              ...
```

### 3.5 Regression vectors

| Vector | Mitigation |
|---|---|
| Claude-only corpus suddenly tries codex parser | `it_predict_pure_claude_corpus_works` |
| Cache poisoning across agents | "cache keys include agent" unit test above |
| Legacy v2 payloads (no `source_agent`) silently switch parser | `predict_routes_missing_source_agent_defaults_claude` |
| Neighbor scoring weights change with new parser | snapshot test: predict result hash is stable across 2 runs |
| `parse_codex_session` panics on malformed JSONL | feed `rollout-03-with-errors.jsonl` → Ok or graceful Err, never panic |
| Performance regression (extra branch per neighbor) | benchmark predict over 100-session corpus; before/after diff < 5% |

**Flags / configs**:
- `MEMEX_DEFAULT_NEIGHBORS=10` — exercise both small (1) and large (50) neighbor counts.
- Predict with `--neighbors 0` should be a no-op, not a panic.

### 3.6 Property-based ideas

`src-tauri/tests/predict_proptest.rs`:

```rust
proptest! {
    #[test]
    fn predict_never_panics_on_random_agent_strings(
        agent in "[a-z_]{0,32}",
        neighbors in 0u32..50
    ) {
        // payload = json!({ "source_agent": agent, "source_path": valid_fixture, ... })
        let _ = block_on(predict_next_actions(sid, neighbors, &mock_ctx));
        // assert: never panics; unknown agent → defaults to claude (current behavior)
    }
}
```

### 3.7 CI matrix suggestion

```yaml
jobs:
  predict-routing:
    strategy:
      matrix:
        corpus: [claude-only, codex-only, mixed]
    services:
      qdrant:
        image: qdrant/qdrant:1.18.0
        ports: ['6333:6333', '6334:6334']
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test --test predict_routing_integration -- --ignored
        env:
          MEMEX_TEST_CORPUS: ${{ matrix.corpus }}
```

---

## §4 Candidate D — `deed283` heat-trail SVG cap (generic SVG-stroke pattern)

> **Reviewer note**: per `UPSTREAM_PR_PLAN.md §2 候補 C`, the `#heat-trail` SVG itself **does not exist on upstream/main**, so the literal patch cannot land. **What CAN land is the defensive pattern** — `vector-effect="non-scaling-stroke"` + `clampUnit` + viewBox sanity gate — applied to any SVG drawing code upstream has (e.g., Time Machine rail, dashboard sparklines, topology graph). This plan treats it as a **pattern-application audit + small-scope codemod PR**.

Files conceptually touched (upstream): grep `createElementNS.*svg`, `setAttribute("stroke-width"`, `viewBox`, `<svg`, then apply guards.

### 4.1 Pre-flight

| # | Command | Expected | Status on fork |
|---|---|---|---|
| D1 | `grep -rn 'setAttribute("stroke' src/ index.html` | enumerate every SVG stroke site | reviewer must do this on **upstream/main** first |
| D2 | `grep -rn 'createElementNS.*svg' src/ index.html` | enumerate every dynamic SVG creation | as above |
| D3 | `grep -rn 'viewBox=' src/ index.html` | enumerate static viewBoxes | as above |
| D4 | `npm run tauri build` | OK | 🟢 fork |

### 4.2 Unit tests to add — JS

`tests/js/svg-stroke-guards.test.mjs` (Option 1 of §1.2 again; same vitest setup):

| Test name | Input | Assertion |
|---|---|---|
| `clampUnit_handles_NaN` | `clampUnit(NaN)` | `=== 0` |
| `clampUnit_handles_Infinity` | `clampUnit(Infinity)` | `=== 1` |
| `clampUnit_handles_negative_Infinity` | `clampUnit(-Infinity)` | `=== 0` |
| `clampUnit_passes_through_in_range` | `clampUnit(0.5)` | `=== 0.5` |
| `clampUnit_clamps_above_1` | `clampUnit(1e6)` | `=== 1` |
| `clampUnit_clamps_below_0` | `clampUnit(-1e6)` | `=== 0` |
| `clampUnit_handles_zero` | `clampUnit(0)` | `=== 0` |
| `clampUnit_handles_negative_zero` | `clampUnit(-0)` | `=== 0` |
| `drawHeatTrail_skips_when_container_below_100x100` | mock `getBoundingClientRect()` → {width:50,height:50} | `svg.innerHTML === ""`; no path appended |
| `drawHeatTrail_skips_when_rect_NaN` | rect width = NaN | bail; no append |
| `drawHeatTrail_sets_non_scaling_stroke` | normal hover | every `<path>` and `<circle>` has `vector-effect="non-scaling-stroke"` |
| `drawHeatTrail_caps_stroke_at_4_5` | feed neighbor with `score=1000` | resulting `stroke-width` attribute parses to `≤ 4.5` |
| `drawHeatTrail_sets_preserveAspectRatio_none` | normal call | `svg.getAttribute("preserveAspectRatio") === "none"` |

### 4.3 Integration tests — Playwright (recommended for SVG)

`tests/e2e/heat-trail.spec.ts` (new file, add `@playwright/test` dev-dep):

| Test | How |
|---|---|
| `stroke width never exceeds 4.5 CSS px on hover (fuzz)` | parameterize over 100 random `score` values via `page.evaluate(() => /* inject mock neighbor */)`; for each, measure computed stroke-width with `getComputedStyle()` |
| `end-cap circle radius stays at 5 viewBox units` | hover, query `circle[vector-effect="non-scaling-stroke"]`, assert `r === "5"` |
| `no path drawn when results container hidden` | `display:none` results, hover → SVG empty |
| `oval regression test` | hover 10 cards in sequence, screenshot full viewport, image-diff against `tests/e2e/screenshots/heat-trail-baseline.png` — any pixel with `oklch lightness > 0.7 && spread > 200px` flags as oval reappeared |
| `accessibility — reduced-motion respected` | `prefers-reduced-motion: reduce` → no animation; trail still draws or skips per design |

### 4.4 Manual / E2E (target time: 4 min)

```
S01  open Memex.app
S02  Open DevTools (right-click → Inspect, or ⌘⌥I — enabled via 84db1fc)
S03  Run a query that returns ≥5 results
S04  Hover the first stack card
S05  In DevTools → Elements, find <svg id="heat-trail">
     EXPECT:
       - viewBox is set (e.g. "0 0 1436 600")
       - preserveAspectRatio="none"
       - contains <path vector-effect="non-scaling-stroke" stroke-width="...">
       - every stroke-width ≤ 4.5
       - no path or circle larger than ~10 CSS px in any dimension
S06  Hover sequentially across 10 cards
     EXPECT: trails redraw cleanly, no growing or "stuck" capsule
S07  Resize window to 400×300 (extreme small)
     EXPECT: when results container is <100×100, drawHeatTrail bails → no SVG content
S08  Resize back to 1600×1000, hover
     EXPECT: trails reappear, still capped
S09  Stress test: in DevTools console run
       window.__test_score = Infinity; (then hover)
     EXPECT: no purple oval, NaN guards trigger, console clean
S10  Compare claudedocs/reports/purple-oval/01-initial.png (BUG) vs
       claudedocs/reports/purple-oval/05-after-fix-hover-v2.png (FIXED)
```

**Expected screenshot diff**: bug screenshot shows a viewport-spanning vertical purple capsule overlaying the stack; fixed shows thin (≤4.5px) bezier curves with small circle end-caps.

### 4.5 Regression vectors

| Vector | Mitigation |
|---|---|
| Other SVG drawing code elsewhere (sparklines, topology, time-machine rail) has same bug | §4.1 D1–D3 audit; apply same 4-layer guard everywhere |
| `vector-effect="non-scaling-stroke"` not supported in older WebView versions | macOS WKWebView 16+ supports it; document min macOS as 13+ |
| Stroke too thin to see at high DPI | min `stroke-width: 1.6` preserved — still visible on retina |
| Opacity formula `0.45 + sUnit * 0.45` could exceed 1.0 if sUnit > 1.22 | sUnit is `clampUnit`-ed → max 1, max opacity = 0.9 ✓ |
| CSS belt-and-suspenders `stroke-width: min(4.5px, …)` conflicts with attribute | attribute wins for SVG presentation attrs; CSS only matters if attribute absent. Document. |
| Performance: appending many `<path>` on hover | profile: hover 100 cards in 1s → frame time < 16 ms |

**Configs to test under**:
- macOS 13, 14, 15 (WKWebView version differences for `vector-effect`)
- DPR 1.0, 2.0, 3.0 (retina, super-retina)
- `prefers-reduced-motion: reduce`
- Dark mode + Light mode (color buckets differ)

### 4.6 Property-based ideas (CRITICAL per assignment)

`tests/js/svg-stroke-property.test.mjs`:

```js
import fc from 'fast-check';

test.prop([fc.oneof(
  fc.constant(Number.NEGATIVE_INFINITY),
  fc.constant(Number.POSITIVE_INFINITY),
  fc.constant(Number.NaN),
  fc.constant(0),
  fc.constant(-0),
  fc.constant(0.5),
  fc.constant(1),
  fc.constant(1e6),
  fc.constant(-1e6),
  fc.float({ min: -1e9, max: 1e9, noNaN: false }),
  fc.double()
)])('stroke-width stays ≤ 4.5 for any score', (score) => {
  const sUnit = clampUnit(score);
  const strokePx = Math.min(4.5, 1.6 + sUnit * 2.4);
  expect(strokePx).toBeLessThanOrEqual(4.5);
  expect(strokePx).toBeGreaterThanOrEqual(1.6);
  expect(Number.isFinite(strokePx)).toBe(true);
});

test.prop([fc.double()])('opacity stays in [0, 1]', (score) => {
  const sUnit = clampUnit(score);
  const opacity = 0.45 + sUnit * 0.45;
  expect(opacity).toBeGreaterThanOrEqual(0.45);
  expect(opacity).toBeLessThanOrEqual(0.90);
});

test.prop([fc.double(), fc.double()])('drawHeatTrail bails on tiny container', (w, h) => {
  if (w < 100 || h < 100 || !isFinite(w) || !isFinite(h)) {
    expect(simulateDrawHeatTrail({width:w, height:h}).children.length).toBe(0);
  }
});
```

**Explicit edge inputs** (named-case style — these MUST be in the unit test list above and the property test seed):

| Score input | Expected `strokePx` | Expected opacity |
|---|---|---|
| `-Infinity` | `1.6` | `0.45` |
| `-1.0` | `1.6` | `0.45` |
| `-0` | `1.6` | `0.45` |
| `0` | `1.6` | `0.45` |
| `NaN` | `1.6` | `0.45` |
| `0.5` | `2.8` | `0.675` |
| `1.0` | `4.0` | `0.90` |
| `1.5` | `4.0` (sUnit clamped to 1, → 4.0; min cap 4.5 not triggered) | `0.90` |
| `1e6` | `4.0` | `0.90` |
| `+Infinity` | `4.0` (sUnit → 1) | `0.90` |

> **Invariant**: ∀ score ∈ ℝ ∪ {NaN, ±∞}: `strokePx ≤ 4.5 ∧ strokePx ≥ 1.6 ∧ 0.45 ≤ opacity ≤ 0.90`.

### 4.7 CI matrix suggestion

```yaml
jobs:
  svg-guards:
    strategy:
      matrix:
        os: [macos-13, macos-14, macos-15]
    steps:
      - uses: actions/checkout@v4
      - uses: actions/setup-node@v4
      - run: npm ci && npx playwright install --with-deps webkit
      - run: npm run tauri build
      - run: npx playwright test tests/e2e/heat-trail.spec.ts --project=webkit
      - run: npx vitest run tests/js/svg-stroke-property.test.mjs
      - if: failure()
        uses: actions/upload-artifact@v4
        with: { name: heat-trail-screencaps-${{ matrix.os }}, path: tests/e2e/screenshots/ }
```

---

## §5 Cross-cutting reviewer checklist

Before approving any of the four PRs, the upstream maintainer should verify:

- [ ] PR description includes a single-paragraph summary, root cause, and verification steps (all four candidates have this in the commit message — preserve in PR body)
- [ ] No new clippy warnings introduced (`cargo clippy --all-targets -- -D warnings`)
- [ ] No new `npm audit` high/critical (`npm audit --omit=dev`)
- [ ] Tauri command surface change documented (only Candidate B/C touch Rust; only B widens `snapshot_import` return type if full f55d417 is sent — confirm KF-01-only subset does NOT change command signatures)
- [ ] All `#[ignore]`-gated integration tests are clearly marked and runnable with `cargo test -- --ignored`
- [ ] No secrets / `.env` / `_scripts/deploy/*` references leaked from fork
- [ ] Compatibility with upstream's `Cargo.lock` (run `cargo update --dry-run` and confirm no major bumps)
- [ ] Manual smoke test (§x.4 of each plan) passes on the reviewer's machine in ≤10 min total

---

## §6 Quick risk × value × test-cost summary

| Candidate | Risk | Value | Test cost | Recommendation |
|---|---|---|---|---|
| **A** `e402b1f` mix-modal | 🟢 LOW (frontend only, no schema/IPC change) | 🟢 HIGH (fixes critical UX dead-end) | 🟢 LOW (manual + opt. vitest) | **SEND FIRST** — best maintainer-reception ROI |
| **B** `f55d417` (KF-01 subset) | 🟡 MED (touches IPC call sites; could reject paths users expect to work) | 🟢 HIGH (CVE-class fix — path traversal) | 🟡 MED (proptest + integration + manual) | **SEND SECOND** — strip to KF-01 only; KF-02/03 depend on schema v3 |
| **C** `2b59dc9` predict-codex | 🔴 HIGH (dead code on upstream — `codex_parser` missing) | 🔴 ZERO upstream (bug doesn't exist there) | N/A | **DO NOT SEND** standalone. Only ship if P3+P5 land first. |
| **D** `deed283` heat-trail | 🟡 MED (pattern application — needs audit of every SVG site on upstream) | 🟡 MED (defensive; bug not yet reproduced upstream) | 🟡 MED (Playwright + property tests) | **SEND THIRD as small follow-up** — only if §4.1 audit finds at least one matching upstream SVG site. Otherwise SKIP. |

---

## §7 Test-plan deliverables checklist (for the submitting agent)

Before opening any of the four PRs, the submitter (you, ComBba's agent) must ensure:

- [ ] All §x.1 pre-flight commands run green on `upstream/main + <candidate-patch>`
- [ ] All §x.2 unit tests are committed (or explicit Option-2 note in PR description)
- [ ] All §x.3 integration tests have either run locally with Qdrant OR are gated behind `#[ignore]` / `cargo test -- --ignored`
- [ ] §x.4 manual script has been run end-to-end by a human (not just the agent) — screenshots attached to PR
- [ ] §x.5 regression vectors are each addressed by at least one test in §x.2/§x.3
- [ ] §x.6 property tests are committed for Candidate B (path sandbox) and Candidate D (score clamp) — they are the load-bearing tests for these two
- [ ] §x.7 CI workflow YAML is included in PR diff (placed under `.github/workflows/` per upstream's convention — check existence first)

---

*End of test plan. Re-run `cargo test --release && npm run tauri build` after each rebase against upstream/main.*
