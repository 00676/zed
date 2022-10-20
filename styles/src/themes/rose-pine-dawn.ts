import chroma from "chroma-js";
import { colorRamp, createColorScheme } from "./common/ramps";

const name = "Rosé Pine Dawn";
const author = "";
const url = "";

const ramps = {
  neutral: chroma
    .scale([
      "#575279",
      "#797593",
      "#9893A5",
      "#B5AFB8",
      "#D3CCCC",
      "#F2E9E1",
      "#FFFAF3",
      "#FAF4ED",
    ])
    .domain([0, 0.35, 0.45, 0.65, 0.7, 0.8, 0.9, 1]),
  red: colorRamp(chroma("#B4637A")),
  orange: colorRamp(chroma("#D7827E")),
  yellow: colorRamp(chroma("#EA9D34")),
  green: colorRamp(chroma("#679967")),
  cyan: colorRamp(chroma("#286983")),
  blue: colorRamp(chroma("#56949F")),
  violet: colorRamp(chroma("#907AA9")),
  magenta: colorRamp(chroma("#79549F")),
};

export const light = createColorScheme(`${name}`, true, ramps);
