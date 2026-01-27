import { useEffect, useCallback, useRef } from 'react';
import {
  Play,
  Pause,
  ChevronLeft,
  ChevronRight,
  ChevronsLeft,
  ChevronsRight,
  RotateCcw,
  CheckCircle,
  XCircle,
  Loader2,
  AlertCircle,
} from 'lucide-react';
import { useDebugger } from '../context';
import type { Workflow } from '../../../types';

interface IntegratedDebugToolbarProps {
  /** Workflows to execute */
  workflows: Workflow[];
  /** Payload for execution */
  payload?: Record<string, unknown>;
  /** Auto-execute on change */
  autoExecute?: boolean;
  /** Callback when execution completes */
  onExecutionComplete?: (trace: import('../../../types').ExecutionTrace) => void;
  /** Callback when execution errors */
  onExecutionError?: (error: string) => void;
  /** Additional CSS class */
  className?: string;
}

/**
 * Compact debug toolbar for integration into the visualizer header.
 * Includes execution status, step counter, and playback controls.
 */
export function IntegratedDebugToolbar({
  workflows,
  payload,
  autoExecute = false,
  onExecutionComplete,
  onExecutionError,
  className = '',
}: IntegratedDebugToolbarProps) {
  const {
    state,
    play,
    pause,
    stop,
    reset,
    stepForward,
    stepBackward,
    goToStep,
    startExecution,
    executeTrace,
    setExecutionError,
    runExecution,
    isAtStart,
    isAtEnd,
    hasTrace,
    totalSteps,
    isEngineReady,
    skipFailedConditions,
    setSkipFailedConditions,
  } = useDebugger();

  const { playbackState, currentStepIndex, isExecuting, executionError, trace } = state;

  // Track last execution to prevent duplicates
  const lastExecutionRef = useRef<{ workflows: string; payload: string } | null>(null);

  // Execute workflows with duplicate prevention
  const handleExecute = useCallback(async () => {
    if (!isEngineReady || workflows.length === 0) return;

    // Check if this is the same execution as last time
    const workflowsJson = JSON.stringify(workflows);
    const payloadJson = JSON.stringify(payload || {});
    const current = { workflows: workflowsJson, payload: payloadJson };

    if (
      lastExecutionRef.current?.workflows === current.workflows &&
      lastExecutionRef.current?.payload === current.payload
    ) {
      return; // Skip duplicate execution
    }

    startExecution();
    try {
      const result = await runExecution(workflows, payload || {});
      if (result) {
        executeTrace(result);
        lastExecutionRef.current = current; // Track successful execution
        onExecutionComplete?.(result);
      }
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : 'Execution failed';
      setExecutionError(errorMessage);
      onExecutionError?.(errorMessage);
    }
  }, [
    isEngineReady,
    workflows,
    payload,
    startExecution,
    runExecution,
    executeTrace,
    setExecutionError,
    onExecutionComplete,
    onExecutionError,
  ]);

  // Reset handler that clears tracking ref to allow re-execution
  const handleReset = useCallback(() => {
    reset();
    lastExecutionRef.current = null; // Allow re-execution after reset
  }, [reset]);

  // Go to initial state (before first step)
  const goToFirst = useCallback(() => {
    if (hasTrace) {
      stop(); // Sets currentStepIndex to -1 (ready state)
    }
  }, [hasTrace, stop]);

  // Go to last step
  const goToLast = useCallback(() => {
    if (hasTrace && totalSteps > 0) {
      goToStep(totalSteps - 1);
    }
  }, [hasTrace, totalSteps, goToStep]);

  // Debounced auto-execute when workflows or payload change
  useEffect(() => {
    if (!autoExecute || !isEngineReady || workflows.length === 0) return;

    const timeoutId = setTimeout(() => {
      handleExecute();
    }, 500); // 500ms debounce

    return () => clearTimeout(timeoutId);
  }, [autoExecute, isEngineReady, workflows, payload]); // eslint-disable-line react-hooks/exhaustive-deps

  // Keyboard shortcuts
  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      // Don't handle if in an input/textarea
      if (
        e.target instanceof HTMLInputElement ||
        e.target instanceof HTMLTextAreaElement
      ) {
        return;
      }

      switch (e.key) {
        case ' ':
          e.preventDefault();
          if (playbackState === 'playing') {
            pause();
          } else if (hasTrace && !isAtEnd) {
            play();
          }
          break;
        case 'ArrowRight':
          e.preventDefault();
          if (hasTrace && !isAtEnd) {
            stepForward();
          }
          break;
        case 'ArrowLeft':
          e.preventDefault();
          if (hasTrace && !isAtStart) {
            stepBackward();
          }
          break;
        case 'Home':
          e.preventDefault();
          if (hasTrace) {
            goToFirst();
          }
          break;
        case 'End':
          e.preventDefault();
          if (hasTrace) {
            goToLast();
          }
          break;
        case 'r':
          if (e.metaKey || e.ctrlKey) {
            e.preventDefault();
            handleReset();
          }
          break;
      }
    },
    [playbackState, hasTrace, isAtEnd, isAtStart, play, pause, stepForward, stepBackward, goToFirst, goToLast, handleReset]
  );

  useEffect(() => {
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [handleKeyDown]);

  // Determine status icon
  const getStatusIcon = () => {
    if (isExecuting) {
      return <Loader2 size={14} className="df-debug-spin" />;
    }
    if (executionError) {
      return <XCircle size={14} className="df-debug-toolbar-status-error" />;
    }
    if (hasTrace) {
      // Check if final message has errors
      const finalStep = trace?.steps[trace.steps.length - 1];
      const hasErrors = finalStep?.message?.errors && finalStep.message.errors.length > 0;
      if (hasErrors) {
        return <AlertCircle size={14} className="df-debug-toolbar-status-warning" />;
      }
      return <CheckCircle size={14} className="df-debug-toolbar-status-success" />;
    }
    return null;
  };

  // Get step display text
  const getStepText = () => {
    if (isExecuting) {
      return 'Executing...';
    }
    if (!hasTrace) {
      return 'Ready';
    }
    if (currentStepIndex < 0) {
      return 'Ready';
    }
    return `Step ${currentStepIndex + 1} / ${totalSteps}`;
  };

  return (
    <div className={`df-debug-toolbar-integrated ${className}`}>
      {/* Step counter / status */}
      <div className="df-debug-toolbar-status">
        {getStatusIcon()}
        <span className="df-debug-toolbar-step-text">{getStepText()}</span>
      </div>

      {/* Playback controls */}
      <div className="df-debug-toolbar-controls">
        <button
          className="df-debug-toolbar-btn"
          onClick={goToFirst}
          disabled={!hasTrace || isAtStart || isExecuting}
          title="First step (Home)"
        >
          <ChevronsLeft size={14} />
        </button>

        <button
          className="df-debug-toolbar-btn"
          onClick={stepBackward}
          disabled={!hasTrace || isAtStart || isExecuting}
          title="Previous step (Left Arrow)"
        >
          <ChevronLeft size={14} />
        </button>

        {playbackState === 'playing' ? (
          <button
            className="df-debug-toolbar-btn df-debug-toolbar-btn-primary"
            onClick={pause}
            disabled={!hasTrace || isExecuting}
            title="Pause (Space)"
          >
            <Pause size={14} />
          </button>
        ) : (
          <button
            className="df-debug-toolbar-btn df-debug-toolbar-btn-primary"
            onClick={play}
            disabled={!hasTrace || isAtEnd || isExecuting}
            title="Play (Space)"
          >
            <Play size={14} />
          </button>
        )}

        <button
          className="df-debug-toolbar-btn"
          onClick={stepForward}
          disabled={!hasTrace || isAtEnd || isExecuting}
          title="Next step (Right Arrow)"
        >
          <ChevronRight size={14} />
        </button>

        <button
          className="df-debug-toolbar-btn"
          onClick={goToLast}
          disabled={!hasTrace || isAtEnd || isExecuting}
          title="Last step (End)"
        >
          <ChevronsRight size={14} />
        </button>
      </div>

      {/* Filter workflows checkbox */}
      <div className="df-debug-toolbar-options">
        <label className="df-debug-toolbar-checkbox-label">
          <input
            type="checkbox"
            checked={skipFailedConditions}
            onChange={(e) => setSkipFailedConditions(e.target.checked)}
            className="df-debug-toolbar-checkbox"
            disabled={isExecuting}
          />
          <span>Filter Workflows</span>
        </label>
      </div>

      {/* Run / Reset buttons */}
      <div className="df-debug-toolbar-actions">
        {hasTrace ? (
          <button
            className="df-debug-toolbar-btn-action df-debug-toolbar-btn-reset"
            onClick={handleReset}
            disabled={isExecuting}
            title="Reset (Ctrl+R)"
          >
            <RotateCcw size={12} />
            <span>Reset</span>
          </button>
        ) : (
          <button
            className="df-debug-toolbar-btn-action df-debug-toolbar-btn-run"
            onClick={handleExecute}
            disabled={!isEngineReady || isExecuting || workflows.length === 0}
            title="Run workflow"
          >
            {isExecuting ? (
              <Loader2 size={12} className="df-debug-spin" />
            ) : (
              <Play size={12} />
            )}
            <span>{isExecuting ? 'Running...' : 'Run'}</span>
          </button>
        )}
      </div>
    </div>
  );
}
