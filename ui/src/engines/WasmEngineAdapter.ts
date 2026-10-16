import { WasmEngine, type JsonValue, type RunOptions, type RunResult } from '@goplasmatic/dataflow-wasm';
import type { DataflowEngine, EngineFactory } from '../types/engine';
import type { Workflow } from '../types/workflow';
import { assertEngineVersion } from './versionCheck';

/**
 * Default WASM engine adapter implementing the DataflowEngine interface.
 *
 * This adapter wraps the @goplasmatic/dataflow-wasm package to provide
 * the standard workflow execution capability. The package must be
 * initialized (`await init()`) before an adapter is built.
 */
export class WasmEngineAdapter implements DataflowEngine {
  private engine: WasmEngine;
  private disposed = false;

  constructor(workflows: Workflow[]) {
    // Every execution path builds an adapter, so this is the one chokepoint
    // where a stale engine can be caught before it produces wrong results.
    assertEngineVersion();
    // A refused definition throws the engine's `EngineError`, `issues` and
    // all, which the debugger shows as it is.
    this.engine = new WasmEngine(workflows as unknown as JsonValue[]);
  }

  /**
   * Run one message. The payload is passed as it is: an object is stored as
   * the object, a string stays a string for a `parse_json` task to read. The
   * run is traced unless the options say otherwise, so the debugger has
   * steps to show.
   */
  async run(payload: unknown, options?: RunOptions): Promise<RunResult> {
    return this.engine.run(payload as JsonValue, { trace: true, ...options });
  }

  dispose(): void {
    // Idempotent: the debugger disposes the previous adapter whenever workflows
    // change, and a rebuild that throws can leave the same adapter reachable
    // for a second dispose. `free()` twice on one wasm handle panics.
    if (this.disposed) return;
    this.disposed = true;
    this.engine.free();
  }
}

/**
 * Default engine factory that creates WasmEngineAdapter instances.
 *
 * Use this as the engineFactory prop for DebuggerProvider when you want
 * the standard WASM-based execution.
 */
export const defaultEngineFactory: EngineFactory = (workflows: Workflow[]) =>
  new WasmEngineAdapter(workflows);
