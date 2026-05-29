# Sample corpus — synthetic Claude Code sessions

A **small, fully synthetic** corpus of 12 Claude Code sessions in the exact
`~/.claude/projects/**/*.jsonl` format Memex indexes. It lets a clean-room
judge feel the spatial-memory value **without indexing their own private
sessions**.

Everything here is invented — fake projects (`acme-api`, `acme-web`,
`ml-pipeline`, `infra`), fake paths (`/Users/dev/projects/...`), fake prompts,
and fake tool output. No real session UUIDs, home paths, or private content.
Regenerate deterministically with:

```bash
python3 scripts/gen-sample-corpus.py
```

## Load it

```bash
# 1. Start Qdrant (see ../../README.md → Quick start, or:)
bash scripts/start-qdrant.sh

# 2. Build the binary once (if you haven't)
cargo build --release --manifest-path src-tauri/Cargo.toml

# 3. Parse + index the corpus into Qdrant
./src-tauri/target/release/memex scan --path examples/sample-corpus --index
# → indexed 12/12 session(s) into 'memex_sessions_v3' (0 error(s))
```

## What the corpus contains

| File | Project | Branch | Has error→fix |
|---|---|---|---|
| 01 | acme-api | feat/jwt-auth | |
| 02 | acme-api | fix/rate-limit | Redis connection refused |
| 03 | acme-api | chore/db-migrate | relation "orders" does not exist |
| 04 | acme-web | feat/login-form | |
| 05 | acme-web | fix/build-fail | Module not found: ./utils |
| 06 | acme-web | style/dark-mode | |
| 07 | ml-pipeline | feat/data-loader | |
| 08 | ml-pipeline | fix/training-nan | RuntimeError returned nan |
| 09 | ml-pipeline | chore/deps | ModuleNotFoundError: torch |
| 10 | infra | feat/dockerize | |
| 11 | infra | fix/ci-cache | cargo build error: linker `cc` not found |
| 12 | infra | chore/terraform | |

Four project clusters with cross-cutting error patterns — enough to make
topology cluster, recall match, and mix steer.

## Try the Qdrant-backed surfaces (CLI)

These are real outputs from this corpus (your scores will match — the corpus
is deterministic):

```bash
BIN=./src-tauri/target/release/memex

# Dense KNN search → top hit is the rate-limit session
$BIN search "rate limiter redis" --limit 3

# Lens: error-weighted multi-vector → surfaces the error→fix sessions
$BIN lens "build error" --error 2.0 --content 1.0 --limit 3
#   5.64  infra        fix/ci-cache    (cargo linker error)
#   5.44  acme-web     fix/build-fail  (module not found)

# Proactive recall → finds the past session that solved a similar error
$BIN recall "cargo build linker error" --limit 3
#   0.93  infra/fix-ci-cache   ← the session that fixed exactly this

# Mix & Match (Discovery API): like the JWT session, unlike the NaN-training one
$BIN mix --pos 046df7e8-c8fb-53ef-9e82-9729b41c4ad3 \
         --neg e1072ed7-3405-54ab-aa90-83c288003cb4 --limit 3

# Topology (Distance Matrix → MST)
$BIN topology --sample 12 --per-point 4 --out /tmp/topo.json
#   topology: 12 node(s), 11 MST edge(s), 1 gap(s)
```

(The session IDs above are stable because the corpus is generated
deterministically; `gen-sample-corpus.py` prints the full id table.)

## Note on `predict` and `replay`

`memex predict` and the GUI **Replay** surface re-read the *source* `.jsonl`
on disk, which passes through Memex's path sandbox (`sec.rs`). The sandbox
only trusts the real session roots `~/.claude/projects` and
`~/.codex/sessions`, so running `predict` against a corpus loaded from this
repo path returns:

```
memex: path outside sandbox: …/examples/sample-corpus/02-...jsonl
```

This is the security sandbox working as designed, not a bug. To also exercise
`predict`/`replay` on the sample corpus, copy it under the Claude root first:

```bash
mkdir -p ~/.claude/projects/memex-sample-corpus
cp examples/sample-corpus/*.jsonl ~/.claude/projects/memex-sample-corpus/
./src-tauri/target/release/memex scan --path ~/.claude/projects/memex-sample-corpus --index
./src-tauri/target/release/memex predict 321dd4d0-ae8c-54f1-a500-12c6f927a0d2 --neighbors 5
#   1  Bash  python train.py --steps 500   from ml-pipeline (turn #6)
```

The five Qdrant primitives the judge path highlights —
**lens / mix / topology / replay / proactive recall** — all work directly
from `examples/sample-corpus` (replay via the GUI on indexed sessions);
only `predict`'s source re-read needs the copy-into-root step above.
