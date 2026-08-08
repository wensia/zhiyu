import type { CSSProperties, HTMLAttributes } from "react"

import { BRAND_ASSETS } from "../brand-assets"

const brandMarkStyle = {
  "--brand-mark-image": `url("${BRAND_ASSETS.markUrl}")`,
} as CSSProperties

export function BrandMark({ className = "", style, ...props }: HTMLAttributes<HTMLSpanElement>) {
  return (
    <span
      aria-hidden="true"
      className={`brand-symbol${className ? ` ${className}` : ""}`}
      style={{ ...brandMarkStyle, ...style }}
      {...props}
    />
  )
}
