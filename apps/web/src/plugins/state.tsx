import type { ReactNode } from "react"

import { PluginStateContext, type PluginState } from "./context"

export function PluginStateProvider({ children, value }: { children: ReactNode; value: PluginState }) {
  return <PluginStateContext.Provider value={value}>{children}</PluginStateContext.Provider>
}
