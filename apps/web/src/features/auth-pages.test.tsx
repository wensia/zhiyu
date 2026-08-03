import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { MemoryRouter } from "react-router-dom"
import { describe, expect, it, vi } from "vitest"

import { api } from "../api/client"
import { RegisterPage } from "./auth-pages"

vi.mock("../api/client", () => ({ api: { register: vi.fn() } }))

describe("RegisterPage", () => {
  it("validates email and password before registration", async () => {
    const user = userEvent.setup()
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } })
    render(<QueryClientProvider client={client}><MemoryRouter><RegisterPage /></MemoryRouter></QueryClientProvider>)
    await user.type(screen.getByLabelText("邮箱"), "bad-email")
    await user.type(screen.getByLabelText("密码"), "short")
    await user.click(screen.getByRole("button", { name: "创建账户" }))
    expect(await screen.findByText("请输入正确的邮箱")).toBeInTheDocument()
    expect(screen.getByText("密码至少 12 个字符")).toBeInTheDocument()
    expect(api.register).not.toHaveBeenCalled()
  })
})
