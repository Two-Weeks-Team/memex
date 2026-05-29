#!/usr/bin/env python3
"""Generate a small, fully synthetic sample corpus in Claude Code session
JSONL format for Memex.

The output is deterministic (fixed UUIDs + timestamps) so the committed
corpus under examples/sample-corpus/ is reproducible and reviewable, and it
contains NO private data — every project, path, prompt and tool output here
is invented. This lets a clean-room judge feel the spatial-memory value
(lens / mix / topology / replay / proactive recall) without touching their
own ~/.claude/projects.

Regenerate with:
    python3 scripts/gen-sample-corpus.py
Load into Qdrant with:
    ./src-tauri/target/release/memex scan --path examples/sample-corpus --index
"""
from __future__ import annotations
import json
import os
import uuid
from datetime import datetime, timedelta, timezone

OUT_DIR = os.path.join(os.path.dirname(__file__), "..", "examples", "sample-corpus")
NS = uuid.UUID("00000000-0000-0000-0000-00000000ce11")  # fixed namespace -> deterministic ids
BASE = datetime(2026, 5, 1, 9, 0, 0, tzinfo=timezone.utc)

# Each session: (project, branch, version, turns)
# A turn is one of:
#   ("user", "text")
#   ("say",  "assistant text")
#   ("tool", "ToolName", {input}, "tool result string")   # error if result starts with "ERROR:"
SESSIONS = [
    ("acme-api", "feat/jwt-auth", "2.1.114", [
        ("user", "Add JWT auth to the login endpoint"),
        ("tool", "Read", {"file_path": "src/auth.js"}, "export function login(req,res){ /* TODO */ }"),
        ("say", "I'll sign a JWT on successful login and verify it in middleware."),
        ("tool", "Edit", {"file_path": "src/auth.js", "old": "/* TODO */", "new": "const t=jwt.sign({uid},SECRET)"}, "edited src/auth.js"),
        ("tool", "Bash", {"command": "npm test -- auth"}, "PASS  auth › signs and verifies token (12ms)"),
        ("say", "Login now returns a signed JWT and the middleware verifies it."),
    ]),
    ("acme-api", "fix/rate-limit", "2.1.114", [
        ("user", "Requests are getting throttled wrong, add a proper rate limiter"),
        ("tool", "Grep", {"pattern": "rateLimit", "path": "src"}, "src/server.js:14: app.use(rateLimit())"),
        ("tool", "Bash", {"command": "node src/server.js"}, "ERROR: Error: Redis connection refused at 127.0.0.1:6379"),
        ("say", "Redis isn't running for the limiter store. I'll fall back to an in-memory store when Redis is absent."),
        ("tool", "Edit", {"file_path": "src/server.js", "old": "new RedisStore()", "new": "redisUp? new RedisStore(): undefined"}, "edited src/server.js"),
        ("tool", "Bash", {"command": "node src/server.js"}, "listening on :3000 (rate limiter: memory store)"),
    ]),
    ("acme-api", "chore/db-migrate", "2.1.123", [
        ("user", "Run the new migration for the orders table"),
        ("tool", "Bash", {"command": "npm run migrate"}, "ERROR: error: relation \"orders\" does not exist"),
        ("say", "The migration ordering is wrong — orders depends on customers which hasn't been created yet."),
        ("tool", "Edit", {"file_path": "migrations/003_orders.sql", "old": "REFERENCES customers", "new": "-- ensure 002 ran first"}, "edited migrations/003_orders.sql"),
        ("tool", "Bash", {"command": "npm run migrate"}, "migrated: 002_customers, 003_orders (2 applied)"),
    ]),
    ("acme-web", "feat/login-form", "2.1.114", [
        ("user", "Build a login form component in React"),
        ("tool", "Write", {"file_path": "src/LoginForm.jsx"}, "wrote src/LoginForm.jsx (42 lines)"),
        ("tool", "Edit", {"file_path": "src/styles.css", "old": "/* forms */", "new": ".login{display:grid;gap:8px}"}, "edited src/styles.css"),
        ("tool", "Bash", {"command": "npm run build"}, "built in 3.4s, 1 chunk 84kb"),
        ("say", "LoginForm renders email + password fields with grid layout."),
    ]),
    ("acme-web", "fix/build-fail", "2.1.123", [
        ("user", "The web build is broken on main"),
        ("tool", "Bash", {"command": "npm run build"}, "ERROR: Module not found: Can't resolve './utils' in src/LoginForm.jsx"),
        ("say", "The helper moved to src/lib/utils. I'll fix the import path."),
        ("tool", "Edit", {"file_path": "src/LoginForm.jsx", "old": "from './utils'", "new": "from './lib/utils'"}, "edited src/LoginForm.jsx"),
        ("tool", "Bash", {"command": "npm run build"}, "built in 3.1s, 1 chunk 84kb"),
    ]),
    ("acme-web", "style/dark-mode", "2.1.123", [
        ("user", "Add a dark mode using CSS variables"),
        ("tool", "Read", {"file_path": "src/tokens.css"}, ":root{--bg:#fff;--fg:#111}"),
        ("tool", "Edit", {"file_path": "src/tokens.css", "old": "--fg:#111}", "new": "--fg:#111} @media(prefers-color-scheme:dark){:root{--bg:#111;--fg:#eee}}"}, "edited src/tokens.css"),
        ("say", "Dark mode now follows the OS preference via prefers-color-scheme."),
    ]),
    ("ml-pipeline", "feat/data-loader", "2.1.114", [
        ("user", "Write a streaming data loader for the parquet shards"),
        ("tool", "Write", {"file_path": "loader.py"}, "wrote loader.py (61 lines)"),
        ("tool", "Bash", {"command": "pytest tests/test_loader.py"}, "3 passed in 0.42s"),
        ("say", "The loader yields batches lazily so memory stays flat over large shards."),
    ]),
    ("ml-pipeline", "fix/training-nan", "2.1.123", [
        ("user", "Training loss goes to NaN after a few hundred steps"),
        ("tool", "Bash", {"command": "python train.py --steps 500"}, "ERROR: RuntimeError: Function 'LogBackward0' returned nan values"),
        ("say", "The learning rate is too high and logits underflow. I'll lower the LR and add gradient clipping."),
        ("tool", "Edit", {"file_path": "train.py", "old": "lr=1e-2", "new": "lr=3e-4  # clip grads"}, "edited train.py"),
        ("tool", "Bash", {"command": "python train.py --steps 500"}, "step 500 loss=0.83 (stable)"),
    ]),
    ("ml-pipeline", "chore/deps", "2.1.123", [
        ("user", "CI can't import torch after the dependency bump"),
        ("tool", "Bash", {"command": "python -c 'import torch'"}, "ERROR: ModuleNotFoundError: No module named 'torch'"),
        ("say", "torch was dropped from requirements during the bump. I'll pin it back."),
        ("tool", "Edit", {"file_path": "requirements.txt", "old": "numpy==2.0", "new": "numpy==2.0\ntorch==2.4.0"}, "edited requirements.txt"),
        ("tool", "Bash", {"command": "pip install -r requirements.txt"}, "Successfully installed torch-2.4.0"),
    ]),
    ("infra", "feat/dockerize", "2.1.114", [
        ("user", "Containerize the api service"),
        ("tool", "Write", {"file_path": "Dockerfile"}, "wrote Dockerfile (multi-stage, 18 lines)"),
        ("tool", "Bash", {"command": "docker build -t acme-api ."}, "Successfully built acme-api:latest (image 142MB)"),
        ("say", "Multi-stage build keeps the runtime image small."),
    ]),
    ("infra", "fix/ci-cache", "2.1.123", [
        ("user", "The Rust CI job fails when linking"),
        ("tool", "Bash", {"command": "cargo build --release"}, "ERROR: cargo build error: linker `cc` not found"),
        ("say", "The CI image lacks a C toolchain. I'll install build-essential before the build step."),
        ("tool", "Edit", {"file_path": ".github/workflows/ci.yml", "old": "- run: cargo build", "new": "- run: apt-get install -y build-essential\n      - run: cargo build"}, "edited .github/workflows/ci.yml"),
        ("tool", "Bash", {"command": "cargo build --release"}, "Compiling acme v0.1.0 ... Finished release [optimized]"),
    ]),
    ("infra", "chore/terraform", "2.1.123", [
        ("user", "Add an S3 bucket for backups in terraform"),
        ("tool", "Edit", {"file_path": "main.tf", "old": "# resources", "new": "resource \"aws_s3_bucket\" \"backups\"{ bucket=\"acme-backups\" }"}, "edited main.tf"),
        ("tool", "Bash", {"command": "terraform plan"}, "Plan: 1 to add, 0 to change, 0 to destroy."),
        ("say", "Backups bucket is ready to apply with versioning enabled."),
    ]),
]


def det_uuid(seed: str) -> str:
    return str(uuid.uuid5(NS, seed))


def build_session(idx: int, project: str, branch: str, version: str, turns):
    cwd = f"/Users/dev/projects/{project}"
    session_id = det_uuid(f"{project}/{branch}")
    ts = BASE + timedelta(days=idx, minutes=0)
    lines = []
    prev_uuid = None
    step = 0

    def emit(rec_type: str, message, seedtag: str):
        nonlocal prev_uuid, step, ts
        u = det_uuid(f"{project}/{branch}/{seedtag}/{step}")
        rec = {
            "type": rec_type,
            "sessionId": session_id,
            "uuid": u,
            "timestamp": ts.strftime("%Y-%m-%dT%H:%M:%SZ"),
            "cwd": cwd,
            "gitBranch": branch,
            "version": version,
            "message": message,
        }
        if prev_uuid is not None:
            rec["parentUuid"] = prev_uuid
        lines.append(json.dumps(rec, ensure_ascii=False))
        prev_uuid = u
        step += 1
        ts = ts + timedelta(seconds=20)

    for turn in turns:
        if turn[0] == "user":
            emit("user", {"role": "user", "content": turn[1]}, "u")
        elif turn[0] == "say":
            emit("assistant", {"role": "assistant", "content": [{"type": "text", "text": turn[1]}]}, "s")
        elif turn[0] == "tool":
            _, name, tool_input, result = turn
            tu_id = det_uuid(f"{project}/{branch}/tool/{step}")
            emit("assistant", {"role": "assistant", "content": [
                {"type": "tool_use", "id": tu_id, "name": name, "input": tool_input}
            ]}, "t")
            is_err = result.startswith("ERROR:")
            content = result[len("ERROR:"):].strip() if is_err else result
            block = {"type": "tool_result", "tool_use_id": tu_id, "content": content}
            if is_err:
                block["is_error"] = True
            emit("user", {"role": "user", "content": [block]}, "r")
    fname = f"{idx:02d}-{project}-{branch.replace('/', '-')}.jsonl"
    return fname, "\n".join(lines) + "\n", session_id


def main():
    out = os.path.normpath(OUT_DIR)
    os.makedirs(out, exist_ok=True)
    manifest = []
    for i, (project, branch, version, turns) in enumerate(SESSIONS, start=1):
        fname, body, sid = build_session(i, project, branch, version, turns)
        with open(os.path.join(out, fname), "w", encoding="utf-8") as f:
            f.write(body)
        manifest.append((fname, project, branch, sid))
    print(f"wrote {len(manifest)} synthetic sessions to {out}")
    for fname, project, branch, sid in manifest:
        print(f"  {fname}  [{project} {branch}]  session_id={sid}")


if __name__ == "__main__":
    main()
