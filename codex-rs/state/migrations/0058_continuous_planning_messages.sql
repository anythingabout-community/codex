CREATE TABLE continuous_planning_messages (
    thread_id TEXT NOT NULL,
    source_id TEXT NOT NULL,
    snapshot TEXT NOT NULL,
    PRIMARY KEY (thread_id, source_id)
);
