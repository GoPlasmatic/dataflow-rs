import type { ExecutionTrace } from './debug';
import type { Workflow } from './workflow';

/**
 * Interface for custom dataflow engines
 *
 * Implement this interface to provide a custom WASM engine for executing
 * workflows with custom functions or plugins.
 */
export interface DataflowEngine {
  /**
   * Process a payload through the engine and return an execution trace
   * for step-by-step debugging.
   */
  processWithTrace(payload: Record<string, unknown>): Promise<ExecutionTrace>;

  /**
   * Optional cleanup method called when the engine is no longer needed.
   * Use this to free WASM memory.
   */
  dispose?(): void;
}

/**
 * Factory function type for creating engine instances.
 *
 * Called when workflows change to create a new engine instance
 * configured with the current workflows.
 */
export type EngineFactory = (workflows: Workflow[]) => DataflowEngine;
