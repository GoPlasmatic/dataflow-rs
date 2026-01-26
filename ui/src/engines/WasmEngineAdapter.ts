import { WasmEngine } from '@goplasmatic/dataflow-wasm';
import type { DataflowEngine, EngineFactory } from '../types/engine';
import type { ExecutionTrace } from '../types/debug';
import type { Workflow } from '../types/workflow';

/**
 * Default WASM engine adapter implementing the DataflowEngine interface.
 *
 * This adapter wraps the @goplasmatic/dataflow-wasm package to provide
 * the standard workflow execution capability.
 */
export class WasmEngineAdapter implements DataflowEngine {
  private engine: WasmEngine;

  constructor(workflows: Workflow[]) {
    const workflowsJson = JSON.stringify(workflows);
    this.engine = new WasmEngine(workflowsJson);
  }

  async processWithTrace(payload: Record<string, unknown>): Promise<ExecutionTrace> {
    const payloadJson = JSON.stringify(payload);
    const traceJson = await this.engine.process_with_trace(payloadJson);
    return JSON.parse(traceJson) as ExecutionTrace;
  }

  dispose(): void {
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
