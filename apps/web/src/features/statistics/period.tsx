import { createContext, useContext, type ReactNode } from "react"

import { monthKeyOf, monthRange } from "./utils"

export type StatisticsPeriod = {
  month: Date
  monthKey: string
  from: string
  to: string
}

const StatisticsPeriodContext = createContext<StatisticsPeriod | undefined>(undefined)

export function StatisticsPeriodProvider({ month, children }: { month: Date; children: ReactNode }) {
  const range = monthRange(month)
  return (
    <StatisticsPeriodContext.Provider value={{ month, monthKey: monthKeyOf(month), ...range }}>
      {children}
    </StatisticsPeriodContext.Provider>
  )
}

// eslint-disable-next-line react-refresh/only-export-components
export function useStatisticsPeriod() {
  const period = useContext(StatisticsPeriodContext)
  if (!period) throw new Error("useStatisticsPeriod must be used inside StatisticsPeriodProvider")
  return period
}
