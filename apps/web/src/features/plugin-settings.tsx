import { useMutation, useQueryClient } from "@tanstack/react-query"
import { useEffect, useState, type ReactNode } from "react"

import { api } from "../api/client"
import type { Plugin, UpdatePluginResult } from "../api/types"
import { Button, ConfirmDialog, InlineNotice, useToast } from "../components/ui"
import { useTopbarSlots } from "../components/topbar-slots"
import { pluginRegistry } from "../plugins/registry"
import { usePluginState } from "../plugins/context"

const pluginsQueryKey = ["plugins"] as const

function updatePluginCache(queryClient: ReturnType<typeof useQueryClient>, result: UpdatePluginResult) {
  queryClient.setQueryData<Plugin[]>(pluginsQueryKey, (current) => current?.map((plugin) => (
    plugin.id === result.id ? { ...plugin, ...result } : plugin
  )))
}

export function PluginDisabledPage({ pluginId }: { pluginId: string }) {
  const queryClient = useQueryClient()
  const toast = useToast()
  const registration = pluginRegistry.find((plugin) => plugin.id === pluginId)
  const mutation = useMutation({
    mutationFn: () => api.updatePlugin(pluginId, true),
    onSuccess: async (result) => {
      updatePluginCache(queryClient, result)
      await queryClient.invalidateQueries({ queryKey: pluginsQueryKey })
      toast({
        title: `${result.name}已开启`,
        description: result.reconciled ? `自检修复了 ${result.reconciled} 项。` : "自检完成，插件可以继续使用。",
        type: "success",
      })
    },
    onError: (error) => toast({ title: "开启失败", description: error.message, type: "error" }),
  })
  return (
    <main className="workspace plugin-disabled-page">
      <div className="plugin-disabled-card">
        <h1>{registration?.name ?? pluginId}插件已关闭</h1>
        <p>页面已隐藏，但历史数据和账本里的长期算账语义都完整保留。</p>
        <Button disabled={mutation.isPending} onClick={() => mutation.mutate()} variant="primary">
          {mutation.isPending ? "正在自检…" : "开启插件"}
        </Button>
      </div>
    </main>
  )
}

export function PluginRoute({ pluginId, children }: { pluginId: string; children: ReactNode }) {
  const { plugins, isLoading } = usePluginState()
  if (isLoading) return <div className="app-loading"><div className="spinner" /><p>正在读取插件状态…</p></div>
  const enabled = plugins?.find((plugin) => plugin.id === pluginId)?.enabled ?? true
  return enabled ? children : <PluginDisabledPage pluginId={pluginId} />
}

export function PluginSettingsPage() {
  const setTopbarSlots = useTopbarSlots()
  const { plugins, isLoading } = usePluginState()
  const queryClient = useQueryClient()
  const toast = useToast()
  const [confirming, setConfirming] = useState<Plugin>()
  useEffect(() => {
    setTopbarSlots({ title: "插件" })
    return () => setTopbarSlots(undefined)
  }, [setTopbarSlots])

  const mutation = useMutation({
    mutationFn: ({ plugin, enabled }: { plugin: Plugin; enabled: boolean }) => api.updatePlugin(plugin.id, enabled),
    onSuccess: async (result) => {
      updatePluginCache(queryClient, result)
      setConfirming(undefined)
      await queryClient.invalidateQueries({ queryKey: pluginsQueryKey })
      toast({
        title: `${result.name}已${result.enabled ? "开启" : "关闭"}`,
        description: result.enabled && result.reconciled ? `自检修复了 ${result.reconciled} 项。` : undefined,
        type: "success",
      })
    },
    onError: (error) => toast({ title: "更新失败", description: error.message, type: "error" }),
  })

  return (
    <main className="workspace plugin-settings-page">
      <header className="workspace-heading">
        <div><p>按需要开启内置功能。关闭只影响功能入口，不会删除任何数据。</p></div>
      </header>
      {isLoading ? <div className="table-state" aria-busy="true">正在读取插件状态…</div> : !plugins ? (
        <InlineNotice type="error">插件状态暂时无法读取，账户、流水和分类仍可正常使用。</InlineNotice>
      ) : (
        <section aria-label="内置插件" className="plugin-settings-list">
          {plugins.map((plugin) => (
            <article className="plugin-settings-item" key={plugin.id}>
              <div>
                <h2>{plugin.name}</h2>
                <p>{plugin.description}</p>
                {plugin.ownsTransactions ? <small>它创建的流水只能在插件里删除</small> : null}
              </div>
              <label className="plugin-switch">
                <input
                  aria-label={`启用${plugin.name}`}
                  checked={plugin.enabled}
                  disabled={mutation.isPending}
                  onChange={(event) => event.target.checked
                    ? mutation.mutate({ plugin, enabled: true })
                    : setConfirming(plugin)}
                  role="switch"
                  type="checkbox"
                />
                <span aria-hidden="true" />
              </label>
            </article>
          ))}
        </section>
      )}
      <ConfirmDialog
        confirmLabel="确认关闭"
        description="关闭后页面隐藏、数据保留，重新开启会先自检。"
        destructive={false}
        onConfirm={() => confirming && mutation.mutate({ plugin: confirming, enabled: false })}
        onOpenChange={(open) => !open && setConfirming(undefined)}
        open={Boolean(confirming)}
        pending={mutation.isPending}
        title={`关闭${confirming?.name ?? "这个"}插件？`}
      />
    </main>
  )
}
