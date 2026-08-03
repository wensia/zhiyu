import { fireEvent, render, screen, within } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { useState } from "react"
import { describe, expect, it } from "vitest"

import { DatePicker } from "./ui"

function ControlledDatePicker({
  initialValue = "2026-08-02",
  minDate,
  maxDate,
  clearable = false,
}: {
  initialValue?: string
  minDate?: string
  maxDate?: string
  clearable?: boolean
}) {
  const [value, setValue] = useState(initialValue)
  return <DatePicker ariaLabel="发生日期" clearable={clearable} maxDate={maxDate} minDate={minDate} onValueChange={setValue} value={value} />
}

describe("DatePicker", () => {
  it("jumps to a distant year without mutating the value while browsing", async () => {
    const user = userEvent.setup()
    render(<ControlledDatePicker />)

    const trigger = screen.getByRole("button", { name: "发生日期" })
    await user.click(trigger)
    const dialog = screen.getByRole("dialog", { name: "选择日期" })

    await user.click(within(dialog).getByRole("button", { name: "选择年月，当前 2026 年 8 月" }))
    await user.click(within(dialog).getByRole("button", { name: "选择年份，当前 2026 年" }))
    await user.click(within(dialog).getByRole("gridcell", { name: "2021 年" }))

    expect(trigger).toHaveTextContent("2026-08-02")
    expect(within(dialog).getByRole("button", { name: "选择年份，当前 2021 年" })).toBeInTheDocument()

    await user.click(within(dialog).getByRole("gridcell", { name: "3 月" }))
    expect(within(dialog).getByRole("grid", { name: "2021 年 3 月日历" })).toBeInTheDocument()
    expect(trigger).toHaveTextContent("2026-08-02")

    await user.click(within(dialog).getByRole("gridcell", { name: "2021-03-15" }))
    expect(trigger).toHaveTextContent("2021-03-15")
    expect(screen.queryByRole("dialog", { name: "选择日期" })).not.toBeInTheDocument()

    await user.click(trigger)
    expect(screen.getByRole("grid", { name: "2021 年 3 月日历" })).toBeInTheDocument()
  })

  it("propagates minimum and maximum dates to year, month, and day choices", async () => {
    const user = userEvent.setup()
    render(<ControlledDatePicker maxDate="2027-05-20" minDate="2025-03-10" />)

    await user.click(screen.getByRole("button", { name: "发生日期" }))
    const dialog = screen.getByRole("dialog", { name: "选择日期" })
    await user.click(within(dialog).getByRole("button", { name: /^选择年月，当前/ }))
    await user.click(within(dialog).getByRole("button", { name: /^选择年份，当前/ }))

    expect(within(dialog).getByRole("gridcell", { name: "2024 年" })).toBeDisabled()
    expect(within(dialog).getByRole("gridcell", { name: "2025 年" })).toBeEnabled()
    expect(within(dialog).getByRole("gridcell", { name: "2028 年" })).toBeDisabled()

    await user.click(within(dialog).getByRole("gridcell", { name: "2025 年" }))
    expect(within(dialog).getByRole("gridcell", { name: "2 月" })).toBeDisabled()
    expect(within(dialog).getByRole("gridcell", { name: "3 月" })).toBeEnabled()

    await user.click(within(dialog).getByRole("gridcell", { name: "3 月" }))
    expect(within(dialog).getByRole("gridcell", { name: "2025-03-09" })).toBeDisabled()
    expect(within(dialog).getByRole("gridcell", { name: "2025-03-10" })).toBeEnabled()
  })

  it("supports month and year paging from the day grid keyboard", async () => {
    const user = userEvent.setup()
    render(<ControlledDatePicker initialValue="2026-12-31" />)

    await user.click(screen.getByRole("button", { name: "发生日期" }))
    const selectedDay = screen.getByRole("gridcell", { name: "2026-12-31" })
    selectedDay.focus()

    fireEvent.keyDown(selectedDay, { key: "PageDown" })
    expect(screen.getByRole("grid", { name: "2027 年 1 月日历" })).toBeInTheDocument()

    const januaryDay = screen.getByRole("gridcell", { name: "2027-01-31" })
    fireEvent.keyDown(januaryDay, { key: "PageUp", shiftKey: true })
    expect(screen.getByRole("grid", { name: "2026 年 1 月日历" })).toBeInTheDocument()
  })

  it("uses a separate keyboard-reachable button to clear the value", async () => {
    const user = userEvent.setup()
    render(<ControlledDatePicker clearable />)

    await user.click(screen.getByRole("button", { name: "清空发生日期" }))
    expect(screen.getByRole("button", { name: "发生日期" })).toHaveTextContent("选择日期")
    expect(screen.queryByRole("dialog", { name: "选择日期" })).not.toBeInTheDocument()
  })
})
