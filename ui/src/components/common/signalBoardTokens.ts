/**
 * Signal Board token values, as JavaScript literals.
 *
 * Monaco owns its own rendering surface and cannot read CSS custom properties,
 * so the editor theme needs real hex strings. This module is the single place
 * those live — keep it in step with the canonical definitions in
 * `src/components/workflow-visualizer/styles/theme.css` and `src/App.css`.
 *
 * Anything that CAN read CSS variables should use `var(--token)` instead of
 * importing from here (see components/colors.ts for that pattern).
 */
export const SIGNAL_BOARD = {
  light: {
    board: '#eef1f5',
    surface: '#ffffff',
    surface2: '#f6f8fb',
    hairline: '#d6dde6',
    hairline2: '#e6ebf1',
    ink: '#0e1826',
    ink2: '#33475e',
    muted: '#5b6a7d',
    faint: '#9aa9ba',
    accent: '#4b56d6',
    accentSoft: '#e7e9fb',
    sigBoolTrue: '#1a7f37',
    sigBoolFalse: '#cf222e',
    sigNumber: '#0959c0',
    sigString: '#8a5a00',
    sigData: '#1b7c83',
    sigNull: '#6e7781',
  },
  dark: {
    board: '#0a0f16',
    surface: '#10161f',
    surface2: '#141b26',
    hairline: '#263140',
    hairline2: '#1d2632',
    ink: '#e6edf5',
    ink2: '#b3c1d1',
    muted: '#8a9cb0',
    faint: '#566172',
    accent: '#7c86f5',
    accentSoft: '#1b2140',
    sigBoolTrue: '#3fb950',
    sigBoolFalse: '#f85149',
    sigNumber: '#58a6ff',
    sigString: '#e3b341',
    sigData: '#39c5cf',
    sigNull: '#8b949e',
  },
} as const;

/** Monaco `rules[].foreground` wants the hex without its leading `#`. */
export const bare = (hex: string): string => hex.replace('#', '');
