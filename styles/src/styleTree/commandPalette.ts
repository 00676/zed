import Theme from "../themes/theme";
import { colors } from "../tokens";
import { text, backgroundColor, border, borderColor } from "./components";

export default function commandPalette(theme: Theme) {
  return {
    keystrokeSpacing: 8,
    key: {
      text: text(theme, "mono", "active", { size: "xs" }),
      cornerRadius: 4,
      background: backgroundColor(theme, "on300"),
      border: border(theme, "secondary"),
      padding: {
        top: 3,
        bottom: 3,
        left: 8,
        right: 8,
      },
      margin: {
        left: 2
      },
    }
  }
}
