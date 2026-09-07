#!/usr/bin/env python3
"""Build a sanitized agtx-compatible store for observer development.

The schema statements below are transcribed from the pinned upstream
``init_project_schema``/``init_global_schema`` at
``d307c4c182dff19a65370a50403185cb826f7f49``. They are duplicated here on
purpose: running upstream's own constructor would need the upstream binary and
would write into whatever ``AGTX_DATA_DIR`` happens to be set to, and the point
of a fixture is a store nobody's real board shares.

Nothing here touches a real agtx installation. Every path is explicit, and the
script refuses to write into a directory that already contains ``index.db``.

    python3 seed-store.py --data-root /tmp/fx/data --project /tmp/fx/work/repo
"""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import sqlite3
import sys

PROJECT_SCHEMA = """
CREATE TABLE tasks (
    id TEXT PRIMARY KEY, title TEXT NOT NULL, description TEXT,
    status TEXT NOT NULL DEFAULT 'backlog', agent TEXT NOT NULL,
    project_id TEXT NOT NULL, session_name TEXT, worktree_path TEXT,
    branch_name TEXT, pr_number INTEGER, pr_url TEXT, plugin TEXT,
    cycle INTEGER NOT NULL DEFAULT 1, referenced_tasks TEXT,
    escalation_note TEXT, base_branch TEXT,
    created_at TEXT NOT NULL, updated_at TEXT NOT NULL
);
CREATE INDEX idx_tasks_status ON tasks(status);
CREATE INDEX idx_tasks_project ON tasks(project_id);
CREATE TABLE transition_requests (
    id TEXT PRIMARY KEY, task_id TEXT NOT NULL, action TEXT NOT NULL,
    requested_at TEXT NOT NULL, processed_at TEXT, error TEXT,
    reason TEXT, claimed_by TEXT
);
CREATE TABLE notifications (
    id TEXT PRIMARY KEY, message TEXT NOT NULL, created_at TEXT NOT NULL,
    task_id TEXT, kind TEXT
);
CREATE TABLE task_runtime (
    task_id TEXT PRIMARY KEY, phase_status TEXT NOT NULL,
    pane_hash TEXT, pane_changed_at TEXT, updated_at TEXT NOT NULL
);
"""

GLOBAL_SCHEMA = """
CREATE TABLE projects (
    id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE,
    github_url TEXT, default_agent TEXT, last_opened TEXT NOT NULL
);
CREATE TABLE running_agents (
    session_name TEXT PRIMARY KEY, project_id TEXT NOT NULL, task_id TEXT NOT NULL,
    agent_name TEXT NOT NULL, started_at TEXT NOT NULL, status TEXT NOT NULL
);
CREATE INDEX idx_running_project ON running_agents(project_id);
CREATE TABLE tui_heartbeat (project_path TEXT PRIMARY KEY, beat_at TEXT NOT NULL);
CREATE TABLE board_watch (project_path TEXT PRIMARY KEY, beat_at TEXT NOT NULL);
CREATE TABLE mobile_devices (
    id TEXT PRIMARY KEY, label TEXT NOT NULL, token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL, last_seen TEXT, session_id TEXT
);
"""

PROJECT_ID = "11111111-2222-3333-4444-555555555555"

TASKS = [
    ("7ac30001", "Wire the retry budget", "running", "claude", 1, None),
    ("7ac30002", "Review the retry budget", "review", "codex", 1, "7ac30001"),
    ("7ac30003", "Backfill the fixtures", "backlog", "gemini", 2, "7ac30001,7ac3ffff"),
]


def path_hash(path: str) -> str:
    """Upstream ``Database::hash_path``: SHA-256, first 8 bytes, 16 hex chars."""
    return hashlib.sha256(path.encode()).digest()[:8].hex()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--data-root", required=True, type=pathlib.Path)
    parser.add_argument("--project", required=True, type=pathlib.Path)
    args = parser.parse_args()

    data_root = args.data_root.resolve()
    project = args.project.resolve()
    if (data_root / "index.db").exists():
        print(f"refusing to seed over an existing store at {data_root}", file=sys.stderr)
        return 2

    (data_root / "projects").mkdir(parents=True, exist_ok=True)
    (project / ".agtx" / "status").mkdir(parents=True, exist_ok=True)

    project_db = data_root / "projects" / f"{path_hash(str(project))}.db"
    conn = sqlite3.connect(project_db)
    conn.executescript(PROJECT_SCHEMA)
    for task_id, title, status, agent, cycle, refs in TASKS:
        conn.execute(
            "INSERT INTO tasks (id, title, status, agent, project_id, worktree_path,"
            " cycle, referenced_tasks, created_at, updated_at)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (task_id, title, status, agent, PROJECT_ID, str(project), cycle, refs,
             "2026-09-01T09:00:00+00:00", "2026-09-01T10:00:00+00:00"),
        )
    conn.execute(
        "INSERT INTO task_runtime VALUES ('7ac30001','working','abc123',"
        "'2026-09-01T10:02:00+00:00','2026-09-01T10:03:00+00:00')"
    )
    conn.execute(
        "INSERT INTO task_runtime VALUES ('7ac30002','ready',NULL,NULL,"
        "'2026-09-01T10:01:00+00:00')"
    )
    conn.execute(
        "INSERT INTO notifications VALUES ('n-0001','phase artifact appeared',"
        "'2026-09-01T10:02:30+00:00','7ac30002','phase_completed')"
    )
    conn.commit()
    conn.close()

    index = sqlite3.connect(data_root / "index.db")
    index.executescript(GLOBAL_SCHEMA)
    index.execute(
        "INSERT INTO projects VALUES (?,?,?,NULL,'claude','2026-09-01T08:00:00+00:00')",
        (PROJECT_ID, "repo", str(project)),
    )
    index.execute(
        "INSERT INTO running_agents VALUES ('task-7ac30001--repo--wire-the-retry',?,"
        "'7ac30001','claude','2026-09-01T09:30:00+00:00','running')",
        (PROJECT_ID,),
    )
    index.commit()
    index.close()

    status_dir = project / ".agtx" / "status"
    (status_dir / "7ac30001.json").write_text(json.dumps({
        "ts": 1788258120, "state": "working",
        "session_id": "01J0SESSIONAAAAAAAAAAAAAA1",
        "transcript_path": "/home/fixture/.claude/projects/x/transcript.jsonl",
        "tool": "Bash", "agent": "claude",
    }))
    (status_dir / "7ac30002.json").write_text(json.dumps({
        "ts": 1788258100, "state": "blocked",
        "session_id": "01J0SESSIONBBBBBBBBBBBBBB2",
        "message": "Allow Bash(rm -rf /)?", "agent": "codex",
    }))

    print(f"seeded {project_db}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
