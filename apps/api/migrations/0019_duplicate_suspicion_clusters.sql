ALTER TABLE duplicate_suspicions
    ADD COLUMN cluster_key TEXT NOT NULL DEFAULT '';

CREATE INDEX idx_duplicate_suspicions_cluster
    ON duplicate_suspicions(user_id, cluster_key)
    WHERE cluster_key <> '';
