import { ColorScheme } from "../themes/common/colorScheme";
import { withOpacity } from "../utils/color";
import { text, border, background, foreground } from "./components";

export default function tabBar(colorScheme: ColorScheme) {
  const height = 32;

  let elevation = colorScheme.lowest;
  let layer = elevation.middle;

  const tab = {
    height,
    background: background(layer),
    border: border(layer, {
      left: true,
      bottom: true,
      overlay: true,
    }),
    iconClose: foreground(layer),
    iconCloseActive: foreground(layer, "base", "active"),
    iconConflict: foreground(layer, "warning"),
    iconDirty: foreground(layer, "info"),
    iconWidth: 8,
    spacing: 8,
    text: text(layer, "sans", { size: "sm" }),
    padding: {
      left: 8,
      right: 8,
    },
    description: {
      margin: { left: 6, top: 1 },
      ...text(layer, "sans", "base", "variant", { size: "2xs" })
    }
  };

  const activePaneActiveTab = {
    ...tab,
    background: background(elevation.top),
    text: text(elevation.top, "sans", { size: "sm" }),
    border: {
      ...tab.border,
      bottom: false
    },
  };

  const inactivePaneInactiveTab = {
    ...tab,
    background: background(layer),
    text: text(layer, "sans", { size: "sm" }),
  };

  const inactivePaneActiveTab = {
    ...tab,
    background: background(layer),
    text: text(layer, "sans", "base", "variant", { size: "sm" }),
    border: {
      ...tab.border,
      bottom: false
    },
  }

  const draggedTab = {
    ...activePaneActiveTab,
    background: withOpacity(tab.background, 0.8),
    border: {
      ...tab.border,
      top: false,
      left: false,
      right: false,
      bottom: false,
    },
    shadow: elevation.above.shadow,
  }

  return {
    height,
    background: background(layer),
    dropTargetOverlayColor: withOpacity(foreground(layer), 0.6),
    border: border(colorScheme.lowest.top, {
      left: true,
      bottom: true,
      overlay: true,
    }),
    activePane: {
      activeTab: activePaneActiveTab,
      inactiveTab: tab,
    },
    inactivePane: {
      activeTab: inactivePaneActiveTab,
      inactiveTab: inactivePaneInactiveTab,
    },
    draggedTab,
    paneButton: {
      color: foreground(layer),
      border: {
        ...tab.border,
      },
      iconWidth: 12,
      buttonWidth: activePaneActiveTab.height,
      hover: {
        color: foreground(layer, "base", "hovered"),
      },
    },
  }
}