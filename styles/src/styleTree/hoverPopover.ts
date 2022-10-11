import { Elevation } from "../themes/common/colorScheme";
import { background, border, text } from "./components";

export default function HoverPopover(elevation: Elevation) {
  let layer = elevation.middle;
  let baseContainer = {
    background: background(layer),
    cornerRadius: 8,
    padding: {
      left: 8,
      right: 8,
      top: 4,
      bottom: 4,
    },
    shadow: elevation.shadow,
    border: border(layer),
    margin: {
      left: -8,
    },
  };

  return {
    container: baseContainer,
    infoContainer: {
      ...baseContainer,
      background: background(layer, "accent"),
      border: border(layer, "accent"),
    },
    warningContainer: {
      ...baseContainer,
      background: background(layer, "warning"),
      border: border(layer, "warning"),
    },
    errorContainer: {
      ...baseContainer,
      background: background(layer, "negative"),
      border: border(layer, "negative"),
    },
    block_style: {
      padding: { top: 4 },
    },
    prose: text(layer, "sans", { size: "sm" }),
    highlight: elevation.ramps.neutral(0.5).alpha(0.2).hex(), // TODO: blend was used here. Replace with something better
  };
}
