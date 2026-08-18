CREATE TABLE plugin_settings (
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  plugin_id TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  updated_at TEXT NOT NULL,
  PRIMARY KEY (user_id, plugin_id)
);
