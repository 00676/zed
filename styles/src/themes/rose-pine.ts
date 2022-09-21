import chroma from "chroma-js";
import { colorRamp, createColorScheme } from "./common/ramps";

const name = "rosé-pine";

const ramps = {
  neutral: chroma.scale([
    "#191724",
    "#1f1d2e",
    "#26233A",
    "#3E3A53",
    "#56526C",
    "#6E6A86",
    "#908CAA",
    "#E0DEF4",
  ]),
  red: colorRamp(chroma("#EB6F92")),
  orange: colorRamp(chroma("#EBBCBA")),
  yellow: colorRamp(chroma("#F6C177")),
  green: colorRamp(chroma("#8DBD8D")),
  cyan: colorRamp(chroma("#409BBE")),
  blue: colorRamp(chroma("#9CCFD8")),
  violet: colorRamp(chroma("#C4A7E7")),
  magenta: colorRamp(chroma("#AB6FE9")),
};

export const dark = createColorScheme(`${name}`, false, ramps);
