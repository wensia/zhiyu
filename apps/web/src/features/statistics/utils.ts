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

export function findFreeSlot(
  occupied: Array<{ x: number; y: number; w: number; h: number }>,
  w: number,
  h: number,
  cols = 12,
): { x: number; y: number } {
  const width = Math.min(cols, Math.max(1, w))
  const bottom = occupied.reduce((value, item) => Math.max(value, item.y + item.h), 0)
  for (let y = 0; y <= bottom; y += 1) {
    for (let x = 0; x <= cols - width; x += 1) {
      const overlaps = occupied.some((item) => (
        x < item.x + item.w
        && x + width > item.x
        && y < item.y + item.h
        && y + h > item.y
      ))
      if (!overlaps) return { x, y }
    }
  }
  return { x: 0, y: bottom }
}

export function tidyLayout<T extends { x: number; y: number; w: number; h: number }>(widgets: T[]): T[] {
  const placed: T[] = []
  return [...widgets].sort((a, b) => a.y - b.y || a.x - b.x).map((widget) => {
    let y = widget.y
    while (y > 0) {
      const overlaps = placed.some((item) => (
        widget.x < item.x + item.w
        && widget.x + widget.w > item.x
        && y - 1 < item.y + item.h
        && y - 1 + widget.h > item.y
      ))
      if (overlaps) break
      y -= 1
    }
    const next = { ...widget, y }
    placed.push(next)
    return next
  })
}
