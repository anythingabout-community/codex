CREATE TABLE thread_supervisors (
    thread_id TEXT PRIMARY KEY REFERENCES threads(id) ON DELETE CASCADE,
    snapshot TEXT NOT NULL
);

CREATE TABLE thread_supervisor_activity (
    thread_id TEXT NOT NULL REFERENCES threads(id) ON DELETE CASCADE,
    sequence INTEGER NOT NULL,
    activity TEXT NOT NULL,
    PRIMARY KEY (thread_id, sequence)
);
