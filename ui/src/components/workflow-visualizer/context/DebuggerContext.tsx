import { createContext, useContext, useReducer, useCallback, useEffect, useRef, ReactNode } from 'react';
import type {
  DebuggerState,
  DebuggerAction,
  ExecutionTrace,
  ExecutionStep,
  Message,
  Workflow,
  DataflowEngine,
  EngineFactory,
} from '../../../types';
import { getMessageAtStep, getChangesAtStep } from '../../../types';
import type { Change } from '../../../types';

/**
 * Initial debugger state
 */
const initialState: DebuggerState = {
  isActive: false,
  trace: null,
  currentStepIndex: -1,
  playbackState: 'stopped',
  playbackSpeed: 500, // 500ms between steps
  inputPayload: null,
  isExecuting: false,
  executionError: null,
  skipFailedConditions: false,
};

/**
 * Get filtered step indices based on skipFailedConditions setting.
 * Returns indices of steps that should be shown during debugging.
 */
function getFilteredStepIndices(trace: ExecutionTrace | null, skipFailedConditions: boolean): number[] {
  if (!trace || trace.steps.length === 0) {
    return [];
  }
  if (!skipFailedConditions) {
    return trace.steps.map((_, i) => i);
  }
  return trace.steps
    .map((step, i) => ({ step, index: i }))
    .filter(({ step }) => step.result !== 'skipped')
    .map(({ index }) => index);
}

/**
 * Debugger reducer
 */
function debuggerReducer(state: DebuggerState, action: DebuggerAction): DebuggerState {
  switch (action.type) {
    case 'ACTIVATE':
      return {
        ...state,
        isActive: true,
      };

    case 'DEACTIVATE':
      return {
        ...initialState,
        inputPayload: state.inputPayload, // Preserve input
      };

    case 'SET_INPUT_PAYLOAD':
      return {
        ...state,
        inputPayload: action.payload,
      };

    case 'START_EXECUTION':
      return {
        ...state,
        isExecuting: true,
        executionError: null,
        trace: null,
        currentStepIndex: -1,
        playbackState: 'stopped',
      };

    case 'EXECUTE_TRACE':
      return {
        ...state,
        isExecuting: false,
        trace: action.trace,
        currentStepIndex: -1, // Start at "ready" state, before step 0
        playbackState: 'paused',
      };

    case 'EXECUTION_ERROR':
      return {
        ...state,
        isExecuting: false,
        executionError: action.error,
      };

    case 'PLAY':
      if (!state.trace || state.trace.steps.length === 0) return state;
      return {
        ...state,
        playbackState: 'playing',
      };

    case 'PAUSE':
      return {
        ...state,
        playbackState: 'paused',
      };

    case 'STOP':
      return {
        ...state,
        playbackState: 'stopped',
        currentStepIndex: -1, // Reset to "ready" state
      };

    case 'RESET':
      return {
        ...state,
        trace: null,
        currentStepIndex: -1,
        playbackState: 'stopped',
        executionError: null,
      };

    case 'STEP_FORWARD': {
      if (!state.trace || state.trace.steps.length === 0) {
        return state;
      }

      const filteredIndices = getFilteredStepIndices(state.trace, state.skipFailedConditions);
      if (filteredIndices.length === 0) {
        return state;
      }

      // Find current position in filtered list
      const currentFilteredPos = filteredIndices.findIndex(i => i === state.currentStepIndex);

      let nextIndex: number;
      if (state.currentStepIndex === -1) {
        // At ready state, go to first filtered step
        nextIndex = filteredIndices[0];
      } else if (currentFilteredPos === -1) {
        // Current step is not in filtered list (shouldn't happen), go to first
        nextIndex = filteredIndices[0];
      } else if (currentFilteredPos >= filteredIndices.length - 1) {
        // At end of filtered steps, pause
        return {
          ...state,
          playbackState: 'paused',
        };
      } else {
        // Move to next filtered step
        nextIndex = filteredIndices[currentFilteredPos + 1];
      }

      return {
        ...state,
        currentStepIndex: nextIndex,
      };
    }

    case 'STEP_BACKWARD': {
      if (!state.trace || state.currentStepIndex <= -1) {
        return state;
      }

      const filteredIndices = getFilteredStepIndices(state.trace, state.skipFailedConditions);
      if (filteredIndices.length === 0) {
        return {
          ...state,
          currentStepIndex: -1,
          playbackState: 'paused',
        };
      }

      // Find current position in filtered list
      const currentFilteredPos = filteredIndices.findIndex(i => i === state.currentStepIndex);

      let prevIndex: number;
      if (currentFilteredPos <= 0) {
        // At or before first filtered step, go to ready state
        prevIndex = -1;
      } else {
        // Move to previous filtered step
        prevIndex = filteredIndices[currentFilteredPos - 1];
      }

      return {
        ...state,
        currentStepIndex: prevIndex,
        playbackState: 'paused',
      };
    }

    case 'GO_TO_STEP':
      if (!state.trace || action.index < 0 || action.index >= state.trace.steps.length) return state;
      return {
        ...state,
        currentStepIndex: action.index,
        playbackState: 'paused', // Pause on manual navigation
      };

    case 'SET_SPEED':
      return {
        ...state,
        playbackSpeed: Math.max(100, Math.min(2000, action.speed)),
      };

    case 'SET_SKIP_FAILED_CONDITIONS': {
      // If enabling filter and current step would be filtered out, move to nearest valid step
      if (action.skip && state.trace && state.currentStepIndex >= 0) {
        const currentStep = state.trace.steps[state.currentStepIndex];
        if (currentStep && currentStep.result === 'skipped') {
          // Find the next non-skipped step, or go to ready state
          const filteredIndices = getFilteredStepIndices(state.trace, true);
          const nextValidIndex = filteredIndices.find(i => i > state.currentStepIndex);
          const prevValidIndex = [...filteredIndices].reverse().find(i => i < state.currentStepIndex);

          return {
            ...state,
            skipFailedConditions: action.skip,
            currentStepIndex: nextValidIndex ?? prevValidIndex ?? -1,
          };
        }
      }
      return {
        ...state,
        skipFailedConditions: action.skip,
      };
    }

    default:
      return state;
  }
}

/**
 * Context value interface
 */
interface DebuggerContextValue {
  state: DebuggerState;
  dispatch: React.Dispatch<DebuggerAction>;
  // Convenience methods
  activate: () => void;
  deactivate: () => void;
  setInputPayload: (payload: Record<string, unknown>) => void;
  executeTrace: (trace: ExecutionTrace) => void;
  startExecution: () => void;
  setExecutionError: (error: string) => void;
  play: () => void;
  pause: () => void;
  stop: () => void;
  reset: () => void;
  stepForward: () => void;
  stepBackward: () => void;
  goToStep: (index: number) => void;
  setSpeed: (speed: number) => void;
  setSkipFailedConditions: (skip: boolean) => void;
  // Engine execution method
  runExecution: (workflows: Workflow[], payload: Record<string, unknown>) => Promise<ExecutionTrace | null>;
  // Computed values
  currentStep: ExecutionStep | null;
  currentMessage: Message | null;
  currentChanges: Change[];
  isAtStart: boolean;
  isAtEnd: boolean;
  hasTrace: boolean;
  progress: number;
  totalSteps: number;
  /** Current position within filtered steps (0-indexed), -1 if at ready state */
  currentFilteredPosition: number;
  /** Array of actual step indices that are shown (for navigation) */
  filteredStepIndices: number[];
  isEngineReady: boolean;
  skipFailedConditions: boolean;
}

const DebuggerContext = createContext<DebuggerContextValue | null>(null);

interface DebuggerProviderProps {
  children: ReactNode;
  /** Initial payload to use for debugging */
  initialPayload?: Record<string, unknown>;
  /** Auto-start in debug mode */
  autoActivate?: boolean;
  /**
   * Factory function to create engine instances when workflows change.
   * Called whenever workflows are updated to create a fresh engine.
   * Use this for custom WASM engines with plugins.
   */
  engineFactory?: EngineFactory;
}

/**
 * Provider component for debugger state
 */
export function DebuggerProvider({
  children,
  initialPayload,
  autoActivate = false,
  engineFactory,
}: DebuggerProviderProps) {
  const [state, dispatch] = useReducer(debuggerReducer, {
    ...initialState,
    inputPayload: initialPayload || null,
    isActive: autoActivate,
  });

  const playbackTimerRef = useRef<number | null>(null);
  const engineRef = useRef<DataflowEngine | null>(null);
  const lastWorkflowsJsonRef = useRef<string | null>(null);

  // Determine if engine is ready for execution
  const isEngineReady = Boolean(engineFactory);

  // Convenience action dispatchers
  const activate = useCallback(() => dispatch({ type: 'ACTIVATE' }), []);
  const deactivate = useCallback(() => dispatch({ type: 'DEACTIVATE' }), []);
  const setInputPayload = useCallback(
    (payload: Record<string, unknown>) => dispatch({ type: 'SET_INPUT_PAYLOAD', payload }),
    []
  );
  const executeTrace = useCallback(
    (trace: ExecutionTrace) => dispatch({ type: 'EXECUTE_TRACE', trace }),
    []
  );
  const startExecution = useCallback(() => dispatch({ type: 'START_EXECUTION' }), []);
  const setExecutionError = useCallback(
    (error: string) => dispatch({ type: 'EXECUTION_ERROR', error }),
    []
  );
  const play = useCallback(() => dispatch({ type: 'PLAY' }), []);
  const pause = useCallback(() => dispatch({ type: 'PAUSE' }), []);
  const stop = useCallback(() => dispatch({ type: 'STOP' }), []);
  const reset = useCallback(() => dispatch({ type: 'RESET' }), []);
  const stepForward = useCallback(() => dispatch({ type: 'STEP_FORWARD' }), []);
  const stepBackward = useCallback(() => dispatch({ type: 'STEP_BACKWARD' }), []);
  const goToStep = useCallback((index: number) => dispatch({ type: 'GO_TO_STEP', index }), []);
  const setSpeed = useCallback((speed: number) => dispatch({ type: 'SET_SPEED', speed }), []);
  const setSkipFailedConditions = useCallback(
    (skip: boolean) => dispatch({ type: 'SET_SKIP_FAILED_CONDITIONS', skip }),
    []
  );

  /**
   * Execute workflows with the provided payload and return the execution trace.
   * Uses engineFactory to create a new engine when workflows change.
   */
  const runExecution = useCallback(
    async (workflows: Workflow[], payload: Record<string, unknown>): Promise<ExecutionTrace | null> => {
      if (workflows.length === 0 || !engineFactory) {
        return null;
      }

      try {
        const workflowsJson = JSON.stringify(workflows);

        // Create new engine if workflows changed or no engine exists
        if (lastWorkflowsJsonRef.current !== workflowsJson || !engineRef.current) {
          // Dispose previous engine
          if (engineRef.current?.dispose) {
            engineRef.current.dispose();
          }
          engineRef.current = engineFactory(workflows);
          lastWorkflowsJsonRef.current = workflowsJson;
        }
        return await engineRef.current.processWithTrace(payload);
      } catch (error) {
        console.error('Execution error:', error);
        throw error;
      }
    },
    [engineFactory]
  );

  // Cleanup engine on unmount
  useEffect(() => {
    return () => {
      if (engineRef.current?.dispose) {
        engineRef.current.dispose();
        engineRef.current = null;
      }
    };
  }, []);

  // Handle playback timer
  useEffect(() => {
    if (state.playbackState === 'playing') {
      playbackTimerRef.current = window.setInterval(() => {
        dispatch({ type: 'STEP_FORWARD' });
      }, state.playbackSpeed);
    } else {
      if (playbackTimerRef.current) {
        clearInterval(playbackTimerRef.current);
        playbackTimerRef.current = null;
      }
    }

    return () => {
      if (playbackTimerRef.current) {
        clearInterval(playbackTimerRef.current);
      }
    };
  }, [state.playbackState, state.playbackSpeed]);

  // Computed values
  const currentStep = state.trace && state.currentStepIndex >= 0
    ? state.trace.steps[state.currentStepIndex]
    : null;

  const currentMessage = state.trace && state.currentStepIndex >= 0
    ? getMessageAtStep(state.trace, state.currentStepIndex)
    : null;

  const currentChanges = state.trace && state.currentStepIndex >= 0
    ? getChangesAtStep(state.trace, state.currentStepIndex)
    : [];

  // Compute filtered indices for accurate step counting
  const filteredStepIndices = getFilteredStepIndices(state.trace, state.skipFailedConditions);
  const totalSteps = filteredStepIndices.length;

  // Find current position within filtered steps
  const currentFilteredPos = state.currentStepIndex >= 0
    ? filteredStepIndices.findIndex(i => i === state.currentStepIndex)
    : -1;

  const isAtStart = state.currentStepIndex <= -1; // -1 is "ready" state (before step 0)
  const isAtEnd = currentFilteredPos >= totalSteps - 1 && currentFilteredPos >= 0;
  const hasTrace = state.trace !== null && totalSteps > 0;
  const progress = totalSteps > 0 && currentFilteredPos >= 0
    ? (currentFilteredPos + 1) / totalSteps
    : 0;

  const value: DebuggerContextValue = {
    state,
    dispatch,
    activate,
    deactivate,
    setInputPayload,
    executeTrace,
    startExecution,
    setExecutionError,
    play,
    pause,
    stop,
    reset,
    stepForward,
    stepBackward,
    goToStep,
    setSpeed,
    setSkipFailedConditions,
    runExecution,
    currentStep,
    currentMessage,
    currentChanges,
    isAtStart,
    isAtEnd,
    hasTrace,
    progress,
    totalSteps,
    currentFilteredPosition: currentFilteredPos,
    filteredStepIndices,
    isEngineReady,
    skipFailedConditions: state.skipFailedConditions,
  };

  return <DebuggerContext.Provider value={value}>{children}</DebuggerContext.Provider>;
}

/**
 * Hook to access debugger context
 */
export function useDebugger() {
  const context = useContext(DebuggerContext);
  if (!context) {
    throw new Error('useDebugger must be used within a DebuggerProvider');
  }
  return context;
}

/**
 * Hook to check if debugger is available (doesn't throw if not in provider)
 */
export function useDebuggerOptional() {
  return useContext(DebuggerContext);
}
