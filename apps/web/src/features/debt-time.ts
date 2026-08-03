const localDate = () => new Intl.DateTimeFormat("en-CA").format(new Date())

export function elapsedCalendarDays(occurredOn: string, referenceOn = localDate()) {
  const dateNumber = (value: string) => {
    const [year, month, day] = value.split("-").map(Number)
    return Date.UTC(year, month - 1, day)
  }
  const interval = Math.floor((dateNumber(referenceOn) - dateNumber(occurredOn)) / 86_400_000)
  return Number.isFinite(interval) ? Math.max(0, interval) : 0
}
