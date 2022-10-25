import { ColorScheme } from "../themes/common/colorScheme";
import { withOpacity } from "../utils/color";
import {
  background,
  border,
  borderColor,
  foreground,
  text,
} from "./components";
import statusBar from "./statusBar";
import tabBar from "./tabBar";

export default function workspace(colorScheme: ColorScheme) {
  const layer = colorScheme.lowest;
  const titlebarPadding = 6;
  const titlebarButton = {
    cornerRadius: 6,
    padding: {
      top: 1,
      bottom: 1,
      left: 8,
      right: 8,
    },
    ...text(layer, "sans", "variant", { size: "xs" }),
    background: background(layer, "variant"),
    border: border(layer),
    hover: {
      ...text(layer, "sans", "variant", "hovered", { size: "xs" }),
      background: background(layer, "variant", "hovered"),
      border: border(layer, "variant", "hovered"),
    },
  };
  const avatarWidth = 18;

  return {
    background: background(layer),
    joiningProjectAvatar: {
      cornerRadius: 40,
      width: 80,
    },
    joiningProjectMessage: {
      padding: 12,
      ...text(layer, "sans", { size: "lg" }),
    },
    externalLocationMessage: {
      background: background(colorScheme.middle, "accent"),
      border: border(colorScheme.middle, "accent"),
      cornerRadius: 6,
      padding: 12,
      margin: { bottom: 8, right: 8 },
      ...text(colorScheme.middle, "sans", "accent", { size: "xs" }),
    },
    leaderBorderOpacity: 0.7,
    leaderBorderWidth: 2.0,
    tabBar: tabBar(colorScheme),
    modal: {
      margin: {
        bottom: 52,
        top: 52,
      },
      cursor: "Arrow",
    },
    sidebar: {
      initialSize: 240,
      border: border(layer, { left: true, right: true }),
    },
    paneDivider: {
      color: borderColor(layer),
      width: 1,
    },
    statusBar: statusBar(colorScheme),
    titlebar: {
      avatarWidth,
      avatarMargin: 8,
      height: 33, // 32px + 1px for overlaid border
      background: background(layer),
      border: border(layer, { bottom: true, overlay: true }),
      padding: {
        left: 80,
        right: titlebarPadding,
      },

      // Project
      title: text(layer, "sans", "variant"),

      // Collaborators
      avatar: {
        cornerRadius: avatarWidth / 2,
        border: {
          color: "#00000088",
          width: 1,
        },
      },
      inactiveAvatar: {
        cornerRadius: avatarWidth / 2,
        border: {
          color: "#00000088",
          width: 1,
        },
        grayscale: true,
      },
      avatarRibbon: {
        height: 3,
        width: 12,
        // TODO: Chore: Make avatarRibbon colors driven by the theme rather than being hard coded.
      },

      // Sign in buttom
      // FlatButton, Variant
      signInPrompt: {
        ...titlebarButton
      },

      // Offline Indicator
      offlineIcon: {
        color: foreground(layer, "variant"),
        width: 16,
        margin: {
          left: titlebarPadding,
        },
        padding: {
          right: 4,
        },
      },

      // Notice that the collaboration server is out of date
      outdatedWarning: {
        ...text(layer, "sans", "warning", { size: "xs" }),
        background: withOpacity(background(layer, "warning"), 0.3),
        border: border(layer, "warning"),
        margin: {
          left: titlebarPadding,
        },
        padding: {
          left: 8,
          right: 8,
        },
        cornerRadius: 6,
      },
      callControl: {
        cornerRadius: 6,
        color: foreground(layer, "variant"),
        iconWidth: 12,
        buttonWidth: 20,
        hover: {
          background: background(layer, "variant", "hovered"),
          color: foreground(layer, "variant", "hovered"),
        },
      },
      toggleContactsButton: {
        margin: { left: 6 },
        cornerRadius: 6,
        color: foreground(layer, "variant"),
        iconWidth: 8,
        buttonWidth: 20,
        active: {
          background: background(layer, "variant", "active"),
          color: foreground(layer, "variant", "active"),
        },
        hover: {
          background: background(layer, "variant", "hovered"),
          color: foreground(layer, "variant", "hovered"),
        },
      },
      toggleContactsBadge: {
        cornerRadius: 3,
        padding: 2,
        margin: { top: 3, left: 3 },
        border: border(layer),
        background: foreground(layer, "accent"),
      },
      shareButton: {
        ...titlebarButton
      }
    },

    toolbar: {
      height: 34,
      background: background(colorScheme.highest),
      border: border(colorScheme.highest, { bottom: true }),
      itemSpacing: 8,
      navButton: {
        color: foreground(colorScheme.highest, "on"),
        iconWidth: 12,
        buttonWidth: 24,
        cornerRadius: 6,
        hover: {
          color: foreground(colorScheme.highest, "on", "hovered"),
          background: background(colorScheme.highest, "on", "hovered"),
        },
        disabled: {
          color: foreground(colorScheme.highest, "on", "disabled"),
        },
      },
      padding: { left: 8, right: 8, top: 4, bottom: 4 },
    },
    breadcrumbs: {
      ...text(layer, "mono", "variant"),
      padding: { left: 6 },
    },
    disconnectedOverlay: {
      ...text(layer, "sans"),
      background: withOpacity(background(layer), 0.8),
    },
    notification: {
      margin: { top: 10 },
      background: background(colorScheme.middle),
      cornerRadius: 6,
      padding: 12,
      border: border(colorScheme.middle),
      shadow: colorScheme.popoverShadow,
    },
    notifications: {
      width: 400,
      margin: { right: 10, bottom: 10 },
    },
    dock: {
      initialSizeRight: 640,
      initialSizeBottom: 480,
      wash_color: withOpacity(background(colorScheme.highest), 0.5),
      panel: {
        border: border(colorScheme.highest),
      },
      maximized: {
        margin: 32,
        border: border(colorScheme.highest, { overlay: true }),
        shadow: colorScheme.modalShadow,
      },
    },
    dropTargetOverlayColor: withOpacity(
      foreground(layer, "variant"),
      0.5
    ),
  };
}
