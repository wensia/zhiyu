import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { useState } from "react"
import { describe, expect, it } from "vitest"

import { CreatableSelect } from "./ui"

const options = [
  { value: "contact-1", label: "阿青" },
  { value: "contact-2", label: "阿岚" },
]

function ControlledCreatableSelect() {
  const [value, setValue] = useState("")
  const [text, setText] = useState("")
  return <>
    <CreatableSelect
      ariaLabel="联系人"
      onSelect={(next) => setValue(next)}
      onTextChange={(next) => { setText(next); setValue("") }}
      options={options}
      placeholder="选择或输入姓名"
      text={text}
      value={value}
    />
    <output data-testid="state">{`${value}|${text}`}</output>
  </>
}

describe("CreatableSelect", () => {
  it("filters options and offers a create row for a new name", async () => {
    const user = userEvent.setup()
    render(<ControlledCreatableSelect />)

    const input = screen.getByRole("combobox", { name: "联系人" })
    await user.click(input)
    expect(screen.getByRole("option", { name: "阿青" })).toBeInTheDocument()

    await user.type(input, "阿青")
    expect(screen.queryByRole("option", { name: /新建/ })).not.toBeInTheDocument()
    expect(screen.getByRole("option", { name: "阿青" })).toBeInTheDocument()

    await user.clear(input)
    await user.type(input, "阿宝")
    expect(screen.queryByRole("option", { name: "阿青" })).not.toBeInTheDocument()
    await user.click(screen.getByRole("option", { name: /新建"阿宝"/ }))

    expect(input).toHaveValue("阿宝")
    expect(screen.getByTestId("state")).toHaveTextContent("|阿宝")
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
  })

  it("selects an existing option and switches back to a new name when typing", async () => {
    const user = userEvent.setup()
    render(<ControlledCreatableSelect />)

    const input = screen.getByRole("combobox", { name: "联系人" })
    await user.click(input)
    await user.click(screen.getByRole("option", { name: "阿岚" }))

    expect(input).toHaveValue("阿岚")
    expect(screen.getByTestId("state")).toHaveTextContent("contact-2|")

    await user.type(input, "x")
    expect(screen.getByTestId("state")).toHaveTextContent("|阿岚x")
  })
})
