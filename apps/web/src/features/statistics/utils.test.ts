import { describe, expect, it } from "vitest"

import { findFreeSlot } from "./utils"

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
