ALTER TABLE duplicate_suspicions ADD COLUMN event_id TEXT
    REFERENCES transaction_events(id) ON DELETE SET NULL;

ALTER TABLE duplicate_suspicions ADD COLUMN revert_payload TEXT NOT NULL DEFAULT '';
