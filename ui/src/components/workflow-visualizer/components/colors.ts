/**
 * Tree icon colours — Signal Board.
 *
 * These are CSS `var()` references rather than literals, applied through an
 * inline `style={{ color }}` on the icon span. Two reasons:
 *
 *  1. They resolve against the live theme, so the tree adapts between the
 *     light and dark boards. The previous hardcoded VSCode hexes did not —
 *     e.g. the pale `#dcdcaa` condition icon sat on a white light-theme
 *     background at roughly 1.6:1.
 *  2. A consumer overriding `--sig-*` on `.df-visualizer-container` re-tints
 *     the tree along with everything else, for free.
 *
 * The mapping follows the SIGNAL rule — a node is coloured by the kind of
 * value it produces — and is kept in step with the `--df-function-*` badge
 * tokens in styles/theme.css.
 */
export const TREE_COLORS = {
  /** Structural container, so it takes the reserved structural accent. */
  workflow: 'var(--accent)',
  /** Emits a boolean that has not been evaluated yet. */
  condition: 'var(--sig-bool-rest)',
  /** The structural spine — matches --df-function-builtin. */
  task: 'var(--sig-number)',
  /** Writes into data — matches --df-function-map. */
  mapping: 'var(--sig-data)',
  /** Emits messages — matches --df-function-validation. */
  validation: 'var(--sig-string)',
  /** A grouping label, carrying no signal of its own. */
  tasks: 'var(--muted)',
  /** A collection of workflows. */
  folder: 'var(--sig-collection)',
} as const;
