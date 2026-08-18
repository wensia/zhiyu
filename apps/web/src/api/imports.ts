import { useQueryClient } from "@tanstack/react-query"

import { api } from "./client"
import type { BindImportAccountInput, CommitImportInput, ImportDetailParams, ImportListParams, UploadImportInput, UpsertImportAccountMappingInput } from "./types"
import { useIdempotentMutation } from "./use-idempotent-mutation"

export const importKeys = {
  all: ["imports"] as const,
  lists: () => [...importKeys.all, "list"] as const,
  list: (params: ImportListParams = {}) => [...importKeys.lists(), params] as const,
  details: () => [...importKeys.all, "detail"] as const,
  detail: (id: string, params: ImportDetailParams = {}) => [...importKeys.details(), id, params] as const,
}

export const importQueries = {
  list: (params: ImportListParams = {}) => ({
    queryKey: importKeys.list(params),
    queryFn: () => api.imports(params),
  }),
  detail: (id: string, params: ImportDetailParams = {}) => ({
    queryKey: importKeys.detail(id, params),
    queryFn: () => api.importDetail(id, params),
    staleTime: 30_000,
  }),
}

const useInvalidateImportWrites = () => {
  const queryClient = useQueryClient()
  return async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: importKeys.all }),
      queryClient.invalidateQueries({ queryKey: ["transactions"] }),
      queryClient.invalidateQueries({ queryKey: ["transaction-summary"] }),
      queryClient.invalidateQueries({ queryKey: ["transaction-categories"] }),
      queryClient.invalidateQueries({ queryKey: ["ledger-accounts"] }),
    ])
  }
}

export function useUploadImport() {
  const queryClient = useQueryClient()
  return useIdempotentMutation({
    mutationFn: (input: UploadImportInput, write) => api.uploadImport(input, write),
    onSuccess: async (detail) => {
      queryClient.setQueryData(importKeys.detail(detail.id), detail)
      queryClient.setQueryData(importKeys.detail(detail.id, { disposition: undefined, direction: undefined, page: 1, pageSize: 20 }), detail)
      await queryClient.invalidateQueries({ queryKey: importKeys.lists() })
    },
  })
}

export function useCommitImport() {
  const invalidate = useInvalidateImportWrites()
  return useIdempotentMutation({
    mutationFn: ({ id, input }: { id: string; input: CommitImportInput }, write) => api.commitImport(id, input, write),
    onSuccess: invalidate,
  })
}

export function useBindImportAccount() {
  const invalidate = useInvalidateImportWrites()
  return useIdempotentMutation({
    mutationFn: ({ id, input }: { id: string; input: BindImportAccountInput }, write) => api.bindImportAccount(id, input, write),
    onSuccess: invalidate,
  })
}

export function useUpsertImportAccountMapping() {
  const invalidate = useInvalidateImportWrites()
  return useIdempotentMutation({
    mutationFn: (input: UpsertImportAccountMappingInput, write) => api.upsertImportAccountMapping(input, write),
    onSuccess: invalidate,
  })
}

export function useDiscardImport() {
  const invalidate = useInvalidateImportWrites()
  return useIdempotentMutation({
    mutationFn: (id: string, write) => api.discardImport(id, write),
    onSuccess: invalidate,
  })
}
