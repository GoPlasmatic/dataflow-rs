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
import type { ExecutionError, Run, RunResult, Workflow } from '../../../types';
import { toExecutionError } from '../../../types';
import { PLAYBACK } from '../constants';

interface IntegratedDebugToolbarProps {
  /** Workflows to execute */
  workflows: Workflow[];
  /** Payload for execution */
  payload?: Record<string, unknown>;
  /** Auto-execute on change */
  autoExecute?: boolean;
  /** Callback when a run completes, on every status */
  onExecutionComplete?: (result: RunResult) => void;
  /** Callback when a run could not start */
  onExecutionError?: (error: ExecutionError) => void;
  /** Additional CSS class */
  className?: string;
}

/**
 * How the run ended, in a few words: its status, a stopped run's error, and
 * how many errors the final message carries.
 */
function describeRun(run: Run, errorCount: number): string {
  const errors = `${errorCount} error${errorCount === 1 ? '' : 's'}`;
  if (run.status === 'completed_with_errors') {
    return `completed with ${errors}`;
  }
  let text = run.status.replace(/_/g, ' ');
  if (run.status === 'stopped' && run.error) {
    text += `: ${run.error.message}`;
  }
  if (errorCount > 0) {
    text += ` · ${errors}`;
  }
  return text;
}

/** The tone of a run's status: green, amber, or red. */
function runTone(run: Run): 'success' | 'warning' | 'error' {
  switch (run.status) {
    case 'completed':
      return 'success';
    case 'completed_with_errors':
      return 'warning';
    default:
      // stopped, timed_out, cancelled: the run did not reach its end.
      return 'error';
  }
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
    executeResult,
    setExecutionError,
    runExecution,
    isAtStart,
    isAtEnd,
    hasTrace,
    totalSteps,
    currentFilteredPosition,
    filteredStepIndices,
    isEngineReady,
    skipFailedConditions,
    setSkipFailedConditions,
  } = useDebugger();

  const { playbackState, isExecuting, executionError, run, message } = state;

  // A run resolves on every status, so `run` is set whenever one finished,
  // trace or not; `hasTrace` is only about having steps to walk.
  const hasResult = run !== null;
  const finalErrorCount = message?.errors.length ?? 0;

  // Track last execution to prevent duplicates
  const lastExecutionRef = useRef<{
    workflows: string;
    payload: string;
    runExecution: typeof runExecution;
  } | null>(null);

  // Execute workflows with duplicate prevention
  const handleExecute = useCallback(async () => {
    if (!isEngineReady || workflows.length === 0) return;

    // Check if this is the same execution as last time
    const workflowsJson = JSON.stringify(workflows);
    const payloadJson = JSON.stringify(payload || {});
    const current = { workflows: workflowsJson, payload: payloadJson, runExecution };

    if (
      lastExecutionRef.current?.workflows === current.workflows &&
      lastExecutionRef.current?.payload === current.payload &&
      lastExecutionRef.current?.runExecution === current.runExecution
    ) {
      return; // Skip duplicate execution
    }

    startExecution();
    try {
      const result = await runExecution(workflows, payload || {});
      if (result) {
        executeResult(result);
        lastExecutionRef.current = current; // Track successful execution
        onExecutionComplete?.(result);
      }
    } catch (err) {
      // The engine refused a definition or an argument, or could not be
      // built: the run never started. A run that failed resolves instead.
      const error = toExecutionError(err);
      setExecutionError(error);
      onExecutionError?.(error);
    }
  }, [
    isEngineReady,
    workflows,
    payload,
    startExecution,
    runExecution,
    executeResult,
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

  // Go to last step (use actual index from filtered list)
  const goToLast = useCallback(() => {
    if (hasTrace && filteredStepIndices.length > 0) {
      goToStep(filteredStepIndices[filteredStepIndices.length - 1]);
    }
  }, [hasTrace, filteredStepIndices, goToStep]);

  // Debounced auto-execute when workflows, payload, or engine change
  useEffect(() => {
    if (!autoExecute || !isEngineReady || workflows.length === 0) return;

    const timeoutId = setTimeout(() => {
      handleExecute();
    }, PLAYBACK.AUTO_EXECUTE_DEBOUNCE_MS);

    return () => clearTimeout(timeoutId);
  }, [autoExecute, isEngineReady, workflows, payload, handleExecute]);

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
    if (run) {
      switch (runTone(run)) {
        case 'success':
          return <CheckCircle size={14} className="df-debug-toolbar-status-success" />;
        case 'warning':
          return <AlertCircle size={14} className="df-debug-toolbar-status-warning" />;
        case 'error':
          return <XCircle size={14} className="df-debug-toolbar-status-error" />;
      }
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
    if (currentFilteredPosition < 0) {
      return 'Ready';
    }
    return `Step ${currentFilteredPosition + 1} / ${totalSteps}`;
  };

  const runSummary = run && !isExecuting ? describeRun(run, finalErrorCount) : null;

  return (
    <div className={`df-debug-toolbar-integrated ${className}`}>
      {/* Step counter / status */}
      <div className="df-debug-toolbar-status" title={runSummary ?? undefined}>
        {getStatusIcon()}
        <span className="df-debug-toolbar-step-text">{getStepText()}</span>
        {runSummary && run && (
          <span className={`df-debug-toolbar-run-status df-debug-toolbar-run-status-${runTone(run)}`}>
            {runSummary}
          </span>
        )}
      </div>

      {/* Why the run could not start. A refused definition lists its issues;
          the summary stays one line and the list opens on demand. */}
      {executionError && !isExecuting && (
        <details className="df-debug-toolbar-error">
          <summary
            className="df-debug-toolbar-error-summary"
            title={`${executionError.code}: ${executionError.message}`}
          >
            <span className="df-debug-toolbar-error-code">{executionError.code}</span>
            <span className="df-debug-toolbar-error-text">{executionError.message}</span>
          </summary>
          <div className="df-debug-toolbar-error-detail">
            <div className="df-debug-toolbar-error-message">{executionError.message}</div>
            {executionError.issues && executionError.issues.length > 0 && (
              <ul className="df-debug-issues">
                {executionError.issues.map((issue, index) => (
                  <li
                    key={index}
                    className={`df-debug-issue df-debug-issue-${issue.severity.toLowerCase()}`}
                  >
                    <code className="df-debug-issue-code">{issue.code}</code>
                    {/* The document itself is the empty path */}
                    <code className="df-debug-issue-path">{issue.path || '(definition)'}</code>
                    <span className="df-debug-issue-message">{issue.message}</span>
                  </li>
                ))}
              </ul>
            )}
          </div>
        </details>
      )}

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
        {hasResult ? (
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
