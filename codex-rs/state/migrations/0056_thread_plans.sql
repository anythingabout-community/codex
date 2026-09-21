CREATE TABLE thread_plan_revisions (
    thread_id TEXT NOT NULL,
    version INTEGER NOT NULL,
    snapshot TEXT NOT NULL,
    checkpoint_at INTEGER NOT NULL,
    PRIMARY KEY (thread_id, version),
    FOREIGN KEY (thread_id) REFERENCES threads(id) ON DELETE CASCADE
);
