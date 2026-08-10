import { useMutation, type UseMutationOptions, type UseMutationResult } from "@tanstack/react-query"
import { useRef } from "react"

import { ApiClientError, type WriteOptions } from "./client"

/**
 * 让写操作在重试时保持同一把幂等键。
 *
 * 服务端用 `Idempotency-Key` 认出重复提交。键必须绑定「这一次用户意图」而不是
 * 「这一次网络请求」，否则下面这条路径会记两笔账：
 *
 *     点保存 ──▶ 服务端写入成功 ──▶ 响应在回程丢了 ──▶ 前端显示失败
 *                                                          │
 *                                        用户再点一次 ◀─────┘
 *                                              │
 *                                    现算新键 ──▶ 服务端认不出 ──▶ 第二笔
 *
 * 但「同一把键」不能无脑保持到底：用户很可能是改了表单内容再提交，那是新意图，
 * 服务端会以 409 `idempotency_mismatch` 拒绝（debts.rs 的 replay_idempotency
 * 比对 request_hash，不一致即冲突）。而 mutationFn 通常从组件闭包读表单状态、
 * 没有 variables 可比对，hook 自己判断不出内容变没变。
 *
 * 所以让服务端来告诉我们：
 *
 *     失败重试，内容没变 ──▶ 同键 ──▶ 服务端返回首次结果，不重复记账
 *     失败重试，内容变了 ──▶ 同键 ──▶ 409 mismatch ──▶ 换新键重试一次 ──▶ 正常提交
 *
 * 两条路径对调用方都是透明的。成功后清空键，下一次提交视为新意图。
 */
export function useIdempotentMutation<TData, TError = ApiClientError, TVariables = void, TOnMutateResult = unknown>(
  options: Omit<UseMutationOptions<TData, TError, TVariables, TOnMutateResult>, "mutationFn"> & {
    mutationFn: (variables: TVariables, write: WriteOptions) => Promise<TData>
  },
): UseMutationResult<TData, TError, TVariables, TOnMutateResult> {
  const keyRef = useRef<string | undefined>(undefined)
  const { mutationFn, onSuccess, ...rest } = options

  return useMutation<TData, TError, TVariables, TOnMutateResult>({
    ...rest,
    mutationFn: async (variables) => {
      keyRef.current ??= crypto.randomUUID()
      try {
        return await mutationFn(variables, { idempotencyKey: keyRef.current })
      } catch (error) {
        if (error instanceof ApiClientError && error.code === "idempotency_mismatch") {
          keyRef.current = crypto.randomUUID()
          return await mutationFn(variables, { idempotencyKey: keyRef.current })
        }
        throw error
      }
    },
    // TanStack v5 的回调签名是四参：第三个是 onMutate 的返回值，第四个才是 context。
    onSuccess: (data, variables, onMutateResult, context) => {
      keyRef.current = undefined
      return onSuccess?.(data, variables, onMutateResult, context)
    },
  })
}
