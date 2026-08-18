import { createContext, useContext } from "react"

import type { Plugin } from "../api/types"

export type PluginState = {
  plugins?: Plugin[]
  isLoading: boolean
}

export const PluginStateContext = createContext<PluginState | undefined>(undefined)

export function usePluginState(): PluginState {
  return useContext(PluginStateContext) ?? { plugins: undefined, isLoading: false }
}

export function usePluginEnabled(pluginId: string) {
  const { plugins } = usePluginState()
  return plugins?.find((plugin) => plugin.id === pluginId)?.enabled ?? true
}
