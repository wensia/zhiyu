#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 /path/to/ledger.db" >&2
  exit 64
fi

db=$1
if [[ ! -f "$db" ]]; then
  echo "database not found: $db" >&2
  exit 66
fi

sql() {
  sqlite3 -readonly -batch -noheader "$db" "$1"
}

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

assert_zero() {
  local label=$1
  local value=$2
  [[ "$value" == "0" ]] || fail "$label = $value"
}

assert_expected() {
  local label=$1
  local actual=$2
  local expected=${3:-}
  [[ -z "$expected" || "$actual" == "$expected" ]] || fail "$label = $actual, expected $expected"
}

schema_version=$(sql "SELECT COALESCE(MAX(version), 0) FROM schema_migrations;")
(( schema_version >= 7 )) || fail "schema version $schema_version is older than ledger-account-card-number migration v7"

for table in ledger_accounts debts debt_addition_events repayment_events debt_balances; do
  present=$(sql "SELECT COUNT(*) FROM sqlite_master WHERE name = '$table' AND type IN ('table', 'view');")
  [[ "$present" == "1" ]] || fail "missing table/view: $table"
done

for table in debts debt_addition_events repayment_events; do
  present=$(sql "SELECT COUNT(*) FROM pragma_table_info('$table') WHERE name = 'account_id';")
  [[ "$present" == "1" ]] || fail "missing $table.account_id"
done

account_type_column=$(sql "SELECT COUNT(*) FROM pragma_table_info('ledger_accounts') WHERE name = 'account_type';")
[[ "$account_type_column" == "1" ]] || fail "missing ledger_accounts.account_type"

account_type_not_null=$(sql "SELECT COUNT(*) FROM pragma_table_info('ledger_accounts') WHERE name = 'account_type' AND \"notnull\" = 1;")
[[ "$account_type_not_null" == "1" ]] || fail "ledger_accounts.account_type must be NOT NULL"

name_source_column=$(sql "SELECT COUNT(*) FROM pragma_table_info('ledger_accounts') WHERE name = 'name_source';")
[[ "$name_source_column" == "1" ]] || fail "missing ledger_accounts.name_source"

name_source_not_null=$(sql "SELECT COUNT(*) FROM pragma_table_info('ledger_accounts') WHERE name = 'name_source' AND \"notnull\" = 1;")
[[ "$name_source_not_null" == "1" ]] || fail "ledger_accounts.name_source must be NOT NULL"

for column in bank_name branch_name card_number nickname phone email; do
  present=$(sql "SELECT COUNT(*) FROM pragma_table_info('ledger_accounts') WHERE name = '$column';")
  [[ "$present" == "1" ]] || fail "missing ledger_accounts.$column"
done

invalid_account_types=$(sql "
  SELECT COUNT(*)
  FROM ledger_accounts
  WHERE TRIM(account_type) = ''
     OR account_type NOT IN (
       'wechat_balance',
       'alipay_balance',
       'bank_card',
       'cash',
       'digital_cny',
       'other'
     );
")
assert_zero "empty or unsupported ledger account types" "$invalid_account_types"

invalid_name_sources=$(sql "
  SELECT COUNT(*)
  FROM ledger_accounts
  WHERE name_source NOT IN ('custom', 'derived');
")
assert_zero "unsupported ledger account name sources" "$invalid_name_sources"

invalid_derived_names=$(sql "
  SELECT COUNT(*)
  FROM ledger_accounts
  WHERE name_source = 'derived'
    AND (
         length(name) > 80
      OR (account_type = 'bank_card' AND (
           COALESCE(card_number, bank_name, branch_name) IS NULL
        OR name <> COALESCE(card_number, bank_name, branch_name)
      ))
      OR (account_type = 'wechat_balance' AND (
           COALESCE(nickname, phone) IS NULL
        OR name <> COALESCE(nickname, phone)
      ))
      OR (account_type = 'alipay_balance' AND (
           COALESCE(nickname, phone, email) IS NULL
        OR name <> COALESCE(nickname, phone, email)
      ))
      OR account_type IN ('cash', 'digital_cny', 'other')
    );
")
assert_zero "derived ledger account names inconsistent with account details" "$invalid_derived_names"

inapplicable_account_details=$(sql "
  SELECT COUNT(*)
  FROM ledger_accounts
  WHERE (account_type = 'bank_card' AND (nickname IS NOT NULL OR phone IS NOT NULL OR email IS NOT NULL))
     OR (account_type = 'wechat_balance' AND (bank_name IS NOT NULL OR branch_name IS NOT NULL OR card_number IS NOT NULL OR email IS NOT NULL))
     OR (account_type = 'alipay_balance' AND (bank_name IS NOT NULL OR branch_name IS NOT NULL OR card_number IS NOT NULL))
     OR (account_type IN ('cash', 'digital_cny', 'other') AND (
          bank_name IS NOT NULL
       OR branch_name IS NOT NULL
       OR card_number IS NOT NULL
       OR nickname IS NOT NULL
       OR phone IS NOT NULL
       OR email IS NOT NULL
     ));
")
assert_zero "account details present on an inapplicable account type" "$inapplicable_account_details"

invalid_account_details=$(sql "
  SELECT COUNT(*)
  FROM ledger_accounts
  WHERE (bank_name IS NOT NULL AND (length(trim(bank_name)) = 0 OR length(trim(bank_name)) > 80))
     OR (branch_name IS NOT NULL AND (length(trim(branch_name)) = 0 OR length(trim(branch_name)) > 120))
     OR (card_number IS NOT NULL AND (length(trim(card_number)) < 12 OR length(trim(card_number)) > 23 OR card_number GLOB '*[^0-9]*'))
     OR (nickname IS NOT NULL AND (length(trim(nickname)) = 0 OR length(trim(nickname)) > 80))
     OR (phone IS NOT NULL AND (length(trim(phone)) < 7 OR length(trim(phone)) > 64))
     OR (email IS NOT NULL AND (length(trim(email)) < 3 OR length(trim(email)) > 254));
")
assert_zero "blank or oversized account details" "$invalid_account_details"

wechat_name_accounts=$(sql "SELECT COUNT(*) FROM ledger_accounts WHERE name = '微信零钱';")
wechat_backfill_violations=$(sql "
  SELECT COUNT(*)
  FROM ledger_accounts
  WHERE name = '微信零钱'
    AND account_type <> 'wechat_balance';
")
assert_zero "微信零钱 backfill type violations" "$wechat_backfill_violations"
assert_expected "微信零钱 account count" "$wechat_name_accounts" "${EXPECTED_WECHAT_BALANCE_ACCOUNTS:-}"

foreign_key_violations=$(sql "SELECT COUNT(*) FROM pragma_foreign_key_check;")
assert_zero "foreign-key violations" "$foreign_key_violations"

cross_user_links=$(sql "
  SELECT
    (SELECT COUNT(*) FROM debts d JOIN ledger_accounts a ON a.id = d.account_id WHERE a.user_id <> d.user_id) +
    (SELECT COUNT(*) FROM debt_addition_events e JOIN ledger_accounts a ON a.id = e.account_id WHERE a.user_id <> e.user_id) +
    (SELECT COUNT(*) FROM repayment_events e JOIN ledger_accounts a ON a.id = e.account_id WHERE a.user_id <> e.user_id);
")
assert_zero "cross-user account links" "$cross_user_links"

balance_violations=$(sql "
  SELECT COUNT(*)
  FROM debts d
  LEFT JOIN debt_balances b ON b.debt_id = d.id
  WHERE b.debt_id IS NULL
     OR b.remaining_cents <> d.principal_cents - b.paid_cents
     OR b.event_count < 0;
")
assert_zero "debt balance violations" "$balance_violations"

event_count_violations=$(sql "
  SELECT COUNT(*)
  FROM debt_balances b
  WHERE b.event_count <>
    (SELECT COUNT(*) FROM repayment_events r WHERE r.debt_id = b.debt_id) +
    (SELECT COUNT(*) FROM debt_addition_events a WHERE a.debt_id = b.debt_id);
")
assert_zero "debt event-count violations" "$event_count_violations"

reversal_account_violations=$(sql "
  SELECT COUNT(*)
  FROM repayment_events reversal
  JOIN repayment_events original ON original.id = reversal.reverses_event_id
  WHERE reversal.kind = 'reversal'
    AND reversal.account_id IS NOT original.account_id;
")
assert_zero "reversal account inheritance violations" "$reversal_account_violations"

IFS='|' read -r debt_count movement_count linked_count null_count principal_cents paid_cents remaining_cents <<<"$(sql "
  SELECT
    (SELECT COUNT(*) FROM debts),
    (SELECT COUNT(*) FROM debts) +
      (SELECT COUNT(*) FROM debt_addition_events) +
      (SELECT COUNT(*) FROM repayment_events),
    (SELECT COUNT(*) FROM debts WHERE account_id IS NOT NULL) +
      (SELECT COUNT(*) FROM debt_addition_events WHERE account_id IS NOT NULL) +
      (SELECT COUNT(*) FROM repayment_events WHERE account_id IS NOT NULL),
    (SELECT COUNT(*) FROM debts WHERE account_id IS NULL) +
      (SELECT COUNT(*) FROM debt_addition_events WHERE account_id IS NULL) +
      (SELECT COUNT(*) FROM repayment_events WHERE account_id IS NULL),
    COALESCE((SELECT SUM(principal_cents) FROM debts), 0),
    COALESCE((SELECT SUM(paid_cents) FROM debt_balances), 0),
    COALESCE((SELECT SUM(remaining_cents) FROM debt_balances), 0);
")"

assert_expected "debt count" "$debt_count" "${EXPECTED_DEBT_COUNT:-}"
assert_expected "movement row count" "$movement_count" "${EXPECTED_MOVEMENT_ROWS:-}"
assert_expected "principal cents" "$principal_cents" "${EXPECTED_PRINCIPAL_CENTS:-}"
assert_expected "paid cents" "$paid_cents" "${EXPECTED_PAID_CENTS:-}"
assert_expected "remaining cents" "$remaining_cents" "${EXPECTED_REMAINING_CENTS:-}"

echo "PASS: ledger-account migration is internally consistent"
echo "database=$db"
echo "schema_version=$schema_version"
echo "wechat_balance_named_accounts=$wechat_name_accounts"
echo "debt_count=$debt_count"
echo "movement_rows=$movement_count"
echo "linked_movements=$linked_count"
echo "legacy_null_movements=$null_count"
echo "principal_cents=$principal_cents"
echo "paid_cents=$paid_cents"
echo "remaining_cents=$remaining_cents"
