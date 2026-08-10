import { createContext, useContext, type ReactNode } from "react"

export type TopbarSlots = { leading?: ReactNode; title?: ReactNode; actions?: ReactNode; edge?: boolean }
export const TopbarSlotContext = createContext<(slots: TopbarSlots | undefined) => void>(() => undefined)
export const useTopbarSlots = () => useContext(TopbarSlotContext)
