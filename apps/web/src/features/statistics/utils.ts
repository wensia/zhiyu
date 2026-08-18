export const yuan = (cents: number) => new Intl.NumberFormat("zh-CN", {
  style: "currency",
  currency: "CNY",
}).format(cents / 100)

export const monthKeyOf = (month: Date) => (
  `${month.getFullYear()}-${String(month.getMonth() + 1).padStart(2, "0")}`
)

export function monthRange(month: Date) {
  const from = `${monthKeyOf(month)}-01`
  const next = new Date(month.getFullYear(), month.getMonth() + 1, 1)
  return { from, to: `${monthKeyOf(next)}-01` }
}

function configRecord(config: unknown): Record<string, unknown> {
  return config && typeof config === "object" && !Array.isArray(config) ? config as Record<string, unknown> : {}
}

export function categoryKind(config: unknown): "expense" | "income" {
  return configRecord(config).kind === "income" ? "income" : "expense"
}

export function accountsUnarchivedOnly(config: unknown) {
  return configRecord(config).unarchivedOnly !== false
}
