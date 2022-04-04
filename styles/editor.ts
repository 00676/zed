import {
  backgroundColor,
  border,
  iconColor,
  player,
  text,
  TextColor,
} from "./components";
import Theme from "./theme";

export default function editor(theme: Theme) {
  const autocompleteItem = {
    cornerRadius: 6,
    padding: {
      bottom: 2,
      left: 6,
      right: 6,
      top: 2,
    },
  };

  function diagnostic(theme: Theme, color: TextColor) {
    return {
      textScaleFactor: 0.857,
      header: {
        border: border(theme, "primary", {
          top: true,
        }),
      },
      message: {
        text: {
          ...text(theme, "sans", color),
          size: 14,
        },
        highlightText: {
          ...text(theme, "sans", color, { weight: "bold" }),
          size: 14,
        },
      },
    };
  }

  return {
    textColor: theme.textColor.secondary.value,
    background: backgroundColor(theme, 300),
    activeLineBackground: theme.editor.line.active.value,
    codeActionsIndicator: iconColor(theme, "secondary"),
    diffBackgroundDeleted: backgroundColor(theme, "error"),
    diffBackgroundInserted: backgroundColor(theme, "ok"),
    documentHighlightReadBackground: theme.editor.highlight.occurrence.value,
    documentHighlightWriteBackground: theme.editor.highlight.occurrence.value,
    errorColor: theme.textColor.error,
    gutterBackground: backgroundColor(theme, 300),
    gutterPaddingFactor: 2.5,
    highlightedLineBackground: theme.editor.line.highlighted.value,
    lineNumber: theme.editor.gutter.primary.value,
    lineNumberActive: theme.editor.gutter.active,
    renameFade: 0.6,
    unnecessaryCodeFade: 0.5,
    selection: player(theme, 1).selection,
    guestSelections: [
      player(theme, 2).selection,
      player(theme, 3).selection,
      player(theme, 4).selection,
      player(theme, 5).selection,
      player(theme, 6).selection,
      player(theme, 7).selection,
      player(theme, 8).selection,
    ],
    autocomplete: {
      background: backgroundColor(theme, 100),
      cornerRadius: 6,
      padding: 6,
      border: border(theme, "secondary"),
      item: autocompleteItem,
      hoveredItem: {
        ...autocompleteItem,
        background: backgroundColor(theme, 100, "hover"),
      },
      margin: {
        left: -14,
      },
      matchHighlight: {
        color: theme.syntax.keyword.color.value,
        weight: theme.syntax.keyword.weight.value,
      },
      selectedItem: {
        ...autocompleteItem,
        background: backgroundColor(theme, 100, "active"),
      },
    },
    diagnosticHeader: {
      background: theme.editor.background.value,
      iconWidthFactor: 1.5,
      textScaleFactor: 0.857,
      border: border(theme, "secondary", {
        bottom: true,
        top: true,
      }),
      code: {
        ...text(theme, "mono", "muted"),
        size: 14,
        margin: {
          left: 10,
        },
      },
      message: {
        highlightText: {
          ...text(theme, "sans", "primary"),
          size: 14,
          weight: "bold",
        },
        text: {
          ...text(theme, "sans", "secondary"),
          size: 14,
        },
      },
    },
    diagnosticPathHeader: {
      background: theme.editor.line.active,
      textScaleFactor: 0.857,
      filename: {
        ...text(theme, "mono", "primary"),
        size: 14,
      },
      path: {
        ...text(theme, "mono", "muted"),
        size: 14,
        margin: {
          left: 12,
        },
      },
    },
    errorDiagnostic: diagnostic(theme, "error"),
    warningDiagnostic: diagnostic(theme, "warning"),
    informationDiagnostic: diagnostic(theme, "info"),
    hintDiagnostic: diagnostic(theme, "info"),
    invalidErrorDiagnostic: diagnostic(theme, "muted"),
    invalidHintDiagnostic: diagnostic(theme, "muted"),
    invalidInformationDiagnostic: diagnostic(theme, "muted"),
    invalidWarningDiagnostic: diagnostic(theme, "muted"),
  };
}
