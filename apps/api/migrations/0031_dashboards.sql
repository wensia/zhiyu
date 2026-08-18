CREATE TABLE dashboards (
    id TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name TEXT NOT NULL CHECK (length(trim(name)) > 0 AND length(name) <= 60),
    position INTEGER NOT NULL CHECK (position >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE UNIQUE INDEX idx_dashboards_user_position
ON dashboards(user_id, position);

CREATE TABLE dashboard_widgets (
    id TEXT PRIMARY KEY,
    dashboard_id TEXT NOT NULL REFERENCES dashboards(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    widget_type TEXT NOT NULL,
    plugin_id TEXT,
    x INTEGER NOT NULL,
    y INTEGER NOT NULL,
    w INTEGER NOT NULL,
    h INTEGER NOT NULL,
    config TEXT NOT NULL DEFAULT '{}',
    position INTEGER NOT NULL CHECK (position >= 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    CHECK (x >= 0 AND y >= 0 AND w BETWEEN 1 AND 12 AND h >= 1 AND x + w <= 12)
);

CREATE INDEX idx_dashboard_widgets_dashboard_position
ON dashboard_widgets(dashboard_id, position);

CREATE INDEX idx_dashboard_widgets_user
ON dashboard_widgets(user_id);
