// Layout constants for resizable panels
export const LAYOUT = {
  LEFT_PANEL: { DEFAULT: 280, MIN: 200, MAX: 450 },
  APP_PANEL: { DEFAULT: 400, MIN: 250, MAX: 600 },
  TREE_HEIGHT_PCT: { DEFAULT: 50, MIN: 20, MAX: 80 },
} as const;

// Playback and timing constants
export const PLAYBACK = {
  DEFAULT_SPEED_MS: 500,
  MIN_SPEED_MS: 100,
  MAX_SPEED_MS: 2000,
  AUTO_EXECUTE_DEBOUNCE_MS: 500,
  AUTO_SCROLL_DELAY_MS: 50,
} as const;

// Tree node ID generation helpers
export const NODE_IDS = {
  ROOT: 'workflows-root',
  workflow: (id: string) => `workflow-${id}`,
  task: (workflowId: string, taskId: string) => `task-${workflowId}-${taskId}`,
  folder: (path: string) => `folder-${path}`,
} as const;
