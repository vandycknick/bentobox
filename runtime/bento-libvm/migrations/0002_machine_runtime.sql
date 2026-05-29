ALTER TABLE machines ADD COLUMN config_json BLOB;

CREATE TABLE IF NOT EXISTS machine_runtime (
    machine_id      TEXT PRIMARY KEY REFERENCES machines(id) ON DELETE CASCADE,
    state           TEXT NOT NULL,
    vmmon_pid       INTEGER,
    started_at      INTEGER,
    last_error      TEXT,
    updated_at      INTEGER NOT NULL
);
