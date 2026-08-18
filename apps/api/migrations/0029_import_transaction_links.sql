INSERT INTO transaction_links(
  id,
  user_id,
  transaction_id,
  plugin_id,
  kind,
  ref_id,
  label,
  created_at
)
SELECT
  'bill-imports:batch:' || r.id,
  b.user_id,
  r.transaction_id,
  'bill-imports',
  'batch',
  b.id,
  CASE b.source_channel
    WHEN 'alipay' THEN '支付宝'
    WHEN 'wechat' THEN '微信支付'
    WHEN 'cmb' THEN '招商银行'
    WHEN 'cmbc' THEN '民生银行'
    ELSE b.source_channel
  END || ' · ' || b.period_start || ' 至 ' || b.period_end,
  COALESCE(b.committed_at, b.updated_at, b.created_at)
FROM import_records r
JOIN import_batches b
  ON b.id = r.batch_id
JOIN ledger_transactions t
  ON t.id = r.transaction_id
 AND t.user_id = b.user_id
WHERE r.transaction_id IS NOT NULL;
