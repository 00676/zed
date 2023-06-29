import {
    chroma,
    color_ramp,
    ThemeAppearance,
    ThemeLicenseType,
    ThemeConfig,
} from "../../common"
import { color as c, syntax } from "./common"

const color = c.default

const green = chroma.mix(color.foam, "#10b981", 0.6, "lab")
const magenta = chroma.mix(color.love, color.pine, 0.5, "lab")

export const theme: ThemeConfig = {
    name: "Rosé Pine",
    author: "edunfelt",
    appearance: ThemeAppearance.Dark,
    license_type: ThemeLicenseType.MIT,
    license_url: "https://github.com/edunfelt/base16-rose-pine-scheme",
    license_file: `${__dirname}/LICENSE`,
    input_color: {
        neutral: chroma.scale([
            color.base,
            color.surface,
            color.highlight_high,
            color.overlay,
            color.muted,
            color.subtle,
            color.text,
        ]),
        red: color_ramp(chroma(color.love)),
        orange: color_ramp(chroma(color.iris)),
        yellow: color_ramp(chroma(color.gold)),
        green: color_ramp(chroma(green)),
        cyan: color_ramp(chroma(color.pine)),
        blue: color_ramp(chroma(color.foam)),
        violet: color_ramp(chroma(color.iris)),
        magenta: color_ramp(chroma(magenta)),
    },
    override: {
        syntax: syntax(color),
    },
}
