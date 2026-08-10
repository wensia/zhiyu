CREATE TABLE bill_inbox_sync_state (
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    jmap_account_id TEXT NOT NULL CHECK (length(trim(jmap_account_id)) > 0),
    email_state TEXT,
    last_attempt_at TEXT,
    last_success_at TEXT,
    last_error_code TEXT,
    last_error_at TEXT,
    PRIMARY KEY(user_id, jmap_account_id)
);

CREATE TABLE bill_inbox_messages (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    jmap_account_id TEXT NOT NULL CHECK (length(trim(jmap_account_id)) > 0),
    jmap_email_id TEXT NOT NULL CHECK (length(trim(jmap_email_id)) > 0),
    configured_address TEXT NOT NULL CHECK (length(trim(configured_address)) > 0),
    raw_blob_id TEXT NOT NULL CHECK (length(trim(raw_blob_id)) > 0),
    message_id_header TEXT,
    from_name TEXT,
    from_email TEXT,
    subject TEXT,
    received_at TEXT NOT NULL,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0 AND size_bytes <= 9007199254740991),
    raw_sha256 TEXT CHECK (raw_sha256 IS NULL OR length(raw_sha256) = 64),
    raw_content BLOB,
    raw_content_blob_id TEXT CHECK (raw_content_blob_id IS NULL OR length(trim(raw_content_blob_id)) > 0),
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'processed', 'ignored', 'error')),
    error_code TEXT,
    source_deleted_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (
        (raw_content IS NULL AND raw_sha256 IS NULL AND raw_content_blob_id IS NULL)
        OR
        (raw_content IS NOT NULL AND raw_sha256 IS NOT NULL AND raw_content_blob_id IS NOT NULL)
    ),
    UNIQUE(user_id, jmap_account_id, jmap_email_id)
);
CREATE INDEX idx_bill_inbox_messages_user_status_received
    ON bill_inbox_messages(user_id, status, received_at DESC, id DESC);
CREATE INDEX idx_bill_inbox_messages_user_received
    ON bill_inbox_messages(user_id, received_at DESC, id DESC);
CREATE INDEX idx_bill_inbox_messages_user_raw_sha256
    ON bill_inbox_messages(user_id, raw_sha256);

CREATE TABLE bill_inbox_attachments (
    id TEXT PRIMARY KEY,
    message_id TEXT NOT NULL REFERENCES bill_inbox_messages(id) ON DELETE CASCADE,
    ordinal INTEGER NOT NULL CHECK (ordinal >= 0),
    part_id TEXT,
    blob_id TEXT NOT NULL CHECK (length(trim(blob_id)) > 0),
    name TEXT,
    media_type TEXT NOT NULL CHECK (length(trim(media_type)) > 0),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0 AND size_bytes <= 9007199254740991),
    UNIQUE(message_id, ordinal)
);
