import { Theme } from "@theme"
import { interactiveTextStyle, size } from "@theme/text"
import { margin } from "@theme/properties"
import { containedText } from "@theme/container"
import { iconButton } from "@components/button"

export default function notification(theme: Theme) {
  const HEADER_PADDING = 8 as const

  const message = containedText({
    theme,
    options: {
      size: size.xs,
      margin: margin(0, HEADER_PADDING),
    },
  })

  const close = iconButton(theme)

  const cta = interactiveTextStyle(theme, {
    size: size.xs,
    margin: margin(6, 0, 6, HEADER_PADDING),
  })

  return {
    message,
    close,
    cta
  }
}
