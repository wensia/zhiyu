CREATE TABLE transaction_links (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
  transaction_id TEXT NOT NULL REFERENCES ledger_transactions(id) ON DELETE CASCADE,
  plugin_id TEXT NOT NULL,
  kind TEXT NOT NULL,
  ref_id TEXT NOT NULL,
  label TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  UNIQUE(transaction_id, plugin_id, kind, ref_id)
);

CREATE INDEX idx_transaction_links_user_transaction
  ON transaction_links(user_id, transaction_id);

CREATE INDEX idx_transaction_links_user_plugin_ref
  ON transaction_links(user_id, plugin_id, ref_id);

INSERT INTO transaction_links(
  id, user_id, transaction_id, plugin_id, kind, ref_id, label, created_at
)
SELECT
  'debts:principal:' || d.id,
  d.user_id,
  d.transaction_id,
  'debts',
  'principal',
  d.id,
  c.display_name,
  d.created_at
FROM debts d
JOIN counterparties c
  ON c.id = d.counterparty_id
 AND c.user_id = d.user_id
WHERE d.transaction_id IS NOT NULL;

INSERT INTO transaction_links(
  id, user_id, transaction_id, plugin_id, kind, ref_id, label, created_at
)
SELECT
  'debts:addition:' || e.id,
  e.user_id,
  e.transaction_id,
  'debts',
  'addition',
  e.debt_id,
  c.display_name,
  e.created_at
FROM debt_addition_events e
JOIN debts d
  ON d.id = e.debt_id
 AND d.user_id = e.user_id
JOIN counterparties c
  ON c.id = d.counterparty_id
 AND c.user_id = d.user_id
WHERE e.transaction_id IS NOT NULL;

INSERT INTO transaction_links(
  id, user_id, transaction_id, plugin_id, kind, ref_id, label, created_at
)
SELECT
  'debts:repayment:' || e.id,
  e.user_id,
  e.transaction_id,
  'debts',
  'repayment',
  e.debt_id,
  c.display_name,
  e.created_at
FROM repayment_events e
JOIN debts d
  ON d.id = e.debt_id
 AND d.user_id = e.user_id
JOIN counterparties c
  ON c.id = d.counterparty_id
 AND c.user_id = d.user_id
WHERE e.transaction_id IS NOT NULL;
