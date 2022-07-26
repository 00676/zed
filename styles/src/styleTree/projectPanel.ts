import Theme from "../themes/common/theme";
import { panel } from "./app";
import { backgroundColor, iconColor, player, text } from "./components";

export default function projectPanel(theme: Theme) {
  return {
    ...panel,
    padding: { left: 12, right: 12, top: 6, bottom: 6 },
    indentWidth: 20,
    entry: {
      height: 24,
      iconColor: iconColor(theme, "muted"),
      iconSize: 8,
      iconSpacing: 8,
      text: text(theme, "mono", "secondary", { size: "sm" }),
      hover: {
        background: backgroundColor(theme, 300, "hovered"),
      },
      active: {
        background: backgroundColor(theme, 300, "active"),
        text: text(theme, "mono", "active", { size: "sm" }),
      },
      activeHover: {
        background: backgroundColor(theme, 300, "active"),
        text: text(theme, "mono", "active", { size: "sm" }),
      },
    },
    cutEntryFade: 0.4,
    ignoredEntryFade: 0.6,
    filenameEditor: {
      background: backgroundColor(theme, "on300"),
      text: text(theme, "mono", "active", { size: "sm" }),
      selection: player(theme, 1).selection,
    },
  };
}
