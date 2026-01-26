import { useState, useCallback, useEffect, useRef } from 'react';
import { CheckCircle, XCircle, ChevronLeft, ChevronRight, Pause, Play, Square } from 'lucide-react';
import { useDebugger } from './workflow-visualizer';
import { WasmEngine } from '@goplasmatic/dataflow-wasm';
import type { Workflow, ExecutionTrace } from '../types';
import { getMessageAtStep } from '../types';

export interface DebugControlsProps {
  workflows: Workflow[];
  payloadText: string;
  payloadError: string | null;
  wasmReady: boolean;
}

export function DebugControls({
  workflows,
  payloadText,
  payloadError,
  wasmReady,
}: DebugControlsProps) {
  const {
    state,
    hasTrace,
    isAtStart,
    isAtEnd,
    totalSteps,
    executeTrace,
    reset,
    stepForward,
    stepBackward,
    play,
    pause,
  } = useDebugger();

  const [executionSuccess, setExecutionSuccess] = useState<boolean | null>(null);
  const lastExecutionRef = useRef<{ workflows: string; payload: string } | null>(null);

  const runDebug = useCallback(async () => {
    if (!wasmReady || workflows.length === 0 || payloadError) return;

    // Check if this is the same execution
    const workflowsJson = JSON.stringify(workflows);
    const current = { workflows: workflowsJson, payload: payloadText };
    if (lastExecutionRef.current?.workflows === current.workflows &&
        lastExecutionRef.current?.payload === current.payload) {
      return; // Skip if same as last execution
    }

    setExecutionSuccess(null);
    reset();

    try {
      const payload = JSON.parse(payloadText);
      const engine = new WasmEngine(workflowsJson);

      // Use process_with_trace for step-by-step debugging
      const traceJson = await engine.process_with_trace(JSON.stringify(payload));
      const trace: ExecutionTrace = JSON.parse(traceJson);

      executeTrace(trace);
      lastExecutionRef.current = current;

      // Check if execution was successful
      const finalMessage = trace.steps.length > 0
        ? getMessageAtStep(trace, trace.steps.length - 1)
        : null;
      setExecutionSuccess(finalMessage ? finalMessage.errors.length === 0 : true);

      engine.free();
    } catch (err) {
      console.error('Execution error:', err);
      setExecutionSuccess(false);
    }
  }, [wasmReady, workflows, payloadText, payloadError, executeTrace, reset]);

  // Auto-run when workflows or payload change
  useEffect(() => {
    if (!wasmReady || workflows.length === 0 || payloadError) return;

    // Debounce the auto-run
    const timeoutId = setTimeout(() => {
      runDebug();
    }, 500);

    return () => clearTimeout(timeoutId);
  }, [wasmReady, workflows, payloadText, payloadError, runDebug]);

  const handleReset = useCallback(() => {
    reset();
    setExecutionSuccess(null);
    // Clear the ref so auto-run will re-execute
    lastExecutionRef.current = null;
  }, [reset]);

  return (
    <div className="debug-controls-inline">
      {executionSuccess !== null && (
        <span className={`execution-status ${executionSuccess ? 'success' : 'error'}`}>
          {executionSuccess ? <CheckCircle size={14} /> : <XCircle size={14} />}
          {executionSuccess ? 'Success' : 'Error'}
        </span>
      )}

      {/* Always show step counter */}
      <span className="step-counter">
        {hasTrace
          ? (state.currentStepIndex >= 0
              ? `Step ${state.currentStepIndex + 1} / ${totalSteps}`
              : `Ready (${totalSteps} steps)`)
          : 'Ready'}
      </span>

      {/* Always show step controls */}
      <div className="step-controls">
        <button
          className="step-btn"
          onClick={stepBackward}
          disabled={!hasTrace || isAtStart}
          title="Previous Step"
        >
          <ChevronLeft size={16} />
        </button>

        {state.playbackState === 'playing' ? (
          <button
            className="step-btn"
            onClick={pause}
            title="Pause"
          >
            <Pause size={14} />
          </button>
        ) : (
          <button
            className="step-btn"
            onClick={play}
            disabled={!hasTrace || isAtEnd}
            title="Play"
          >
            <Play size={14} />
          </button>
        )}

        <button
          className="step-btn"
          onClick={stepForward}
          disabled={!hasTrace || isAtEnd}
          title="Next Step"
        >
          <ChevronRight size={16} />
        </button>

        <button
          className="step-btn"
          onClick={handleReset}
          disabled={!hasTrace}
          title="Stop"
        >
          <Square size={12} />
        </button>
      </div>

    </div>
  );
}
