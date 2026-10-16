import type { RunOptions, RunResult } from '@goplasmatic/dataflow-wasm';
import type { Workflow } from './workflow';

/**
 * What the debugger runs a message through: the wasm engine by default
 * (`WasmEngineAdapter`), or a host's own, such as a server-side engine with
 * its registered functions.
 *
 * `run` takes any payload and the engine's run options, and resolves with
 * the engine's `RunResult` on every status: a run that stops, times out or
 * fails resolves too, with `run.status` saying how and `trace` holding the
 * failing step. Pass `trace: true` (the adapter's default) for the debugger
 * to have something to step through.
 */
export interface DataflowEngine {
  /** Run one message and resolve with the message, the run and its trace. */
  run(payload: unknown, options?: RunOptions): Promise<RunResult>;
  /** Release the engine's resources. Called when the workflows change. */
  dispose?(): void;
}

/**
 * Factory function type for creating engine instances.
 * Called whenever workflows change to create a fresh engine with the new workflows.
 *
 * May throw: a definition the engine refuses throws an `EngineError` whose
 * `issues` say what and where, which the debugger shows.
 */
export type EngineFactory = (workflows: Workflow[]) => DataflowEngine;
