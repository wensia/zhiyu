import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { useState } from "react"
import { describe, expect, it } from "vitest"

import { CreatableSelect, Field, Modal } from "./ui"

const options = [
  { value: "contact-1", label: "阿青" },
  { value: "contact-2", label: "阿岚" },
]

function ControlledCreatableSelect() {
  const [value, setValue] = useState("")
  const [text, setText] = useState("")
  // Callers wrap the control in a Field, which renders a <label>: clicking its
  // blank space hands focus straight back to the input.
  return <>
    <Field label="联系人">
      <CreatableSelect
        ariaLabel="联系人"
        onSelect={(next) => setValue(next)}
        onTextChange={(next) => { setText(next); setValue("") }}
        options={options}
        placeholder="选择或输入姓名"
        text={text}
        value={value}
      />
    </Field>
    <output data-testid="state">{`${value}|${text}`}</output>
    {/* Clicking blank chrome does not always blur the input — a popup list
        deliberately preventDefaults on mousedown to keep focus put. */}
    <div data-testid="outside" onMouseDown={(event) => event.preventDefault()}>弹窗空白</div>
  </>
}

function ModalCreatableSelect() {
  return (
    <Modal onOpenChange={() => {}} open title="编辑记账">
      <ControlledCreatableSelect />
    </Modal>
  )
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

  it("closes the popup when a blank area outside is clicked, and reopens on the next click", async () => {
    const user = userEvent.setup()
    render(<ControlledCreatableSelect />)

    const input = screen.getByRole("combobox", { name: "联系人" })
    await user.click(input)
    expect(screen.getByRole("listbox")).toBeInTheDocument()

    await user.click(screen.getByTestId("outside"))
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()

    await user.click(input)
    expect(screen.getByRole("listbox")).toBeInTheDocument()
  })

  it("closes the popup when the blank space of its own field label is clicked", async () => {
    const user = userEvent.setup()
    render(<ModalCreatableSelect />)

    const input = screen.getByRole("combobox", { name: "联系人" })
    await user.click(input)
    expect(screen.getByRole("listbox")).toBeInTheDocument()

    // The label hands focus back to the input, so the popup must not treat that
    // returning focus as a reason to reopen.
    await user.click(screen.getByText("联系人"))
    expect(screen.queryByRole("listbox")).not.toBeInTheDocument()
    expect(input).toHaveFocus()
  })
})
