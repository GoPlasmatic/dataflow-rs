import type { EngineFactory, ExecutionTrace } from './index';

/**
 * Configuration for integrated debug mode in WorkflowVisualizer.
 *
 * When enabled, the visualizer wraps itself with a DebuggerProvider and
 * shows integrated debug controls in the header.
 */
export interface DebugConfig {
  /**
   * Enable integrated debug mode.
   * When true, the visualizer will include a DebuggerProvider and show debug controls.
   */
  enabled: boolean;

  /**
   * Factory function to create engine instances.
   * Required for execution. If not provided, the run button will be disabled.
   */
  engineFactory?: EngineFactory;

  /**
   * Initial payload to use for debugging.
   * Can also be provided via the debugPayload prop on WorkflowVisualizer.
   */
  initialPayload?: Record<string, unknown>;

  /**
   * Automatically execute when workflows or payload change.
   * @default false
   */
  autoExecute?: boolean;

  /**
   * Callback fired when execution completes successfully.
   */
  onExecutionComplete?: (trace: ExecutionTrace) => void;

  /**
   * Callback fired when execution encounters an error.
   */
  onExecutionError?: (error: string) => void;
}
