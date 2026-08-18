import { describe, expect, it } from "vitest"

import { findFreeSlot, tidyLayout } from "./utils"

describe("findFreeSlot", () => {
  it("places the first widget at the origin", () => {
    expect(findFreeSlot([], 4, 3)).toEqual({ x: 0, y: 0 })
  })

  it("fills a hole at the top left", () => {
    expect(findFreeSlot([{ x: 4, y: 0, w: 8, h: 2 }], 4, 2)).toEqual({ x: 0, y: 0 })
  })

  it("moves to the next row when the first row is full", () => {
    expect(findFreeSlot([{ x: 0, y: 0, w: 12, h: 1 }], 3, 1)).toEqual({ x: 0, y: 1 })
  })

  it("clamps widths larger than twelve columns", () => {
    expect(findFreeSlot([{ x: 0, y: 1, w: 12, h: 1 }], 20, 1)).toEqual({ x: 0, y: 0 })
  })

  it("falls back to the occupied bottom when no earlier slot fits", () => {
    expect(findFreeSlot([
      { x: 0, y: 0, w: 12, h: 1 },
      { x: 0, y: 1, w: 12, h: 1 },
    ], 4, 1)).toEqual({ x: 0, y: 2 })
  })
})

describe("tidyLayout", () => {
  it("moves a layout with holes upward without overlaps or changing columns", () => {
    const widgets = [
      { id: "lower-left", x: 0, y: 6, w: 6, h: 2 },
      { id: "upper-right", x: 6, y: 3, w: 6, h: 2 },
      { id: "lower-right", x: 6, y: 8, w: 6, h: 2 },
    ]

    const tidied = tidyLayout(widgets)

    expect(tidied.map(({ id, x, y }) => ({ id, x, y }))).toEqual([
      { id: "upper-right", x: 6, y: 0 },
      { id: "lower-left", x: 0, y: 0 },
      { id: "lower-right", x: 6, y: 2 },
    ])
    for (const [index, widget] of tidied.entries()) {
      expect(tidied.slice(index + 1).every((other) => (
        widget.x + widget.w <= other.x
        || other.x + other.w <= widget.x
        || widget.y + widget.h <= other.y
        || other.y + other.h <= widget.y
      ))).toBe(true)
    }
    expect(Object.fromEntries(tidied.map(({ id, x }) => [id, x]))).toEqual(Object.fromEntries(widgets.map(({ id, x }) => [id, x])))
  })
})
