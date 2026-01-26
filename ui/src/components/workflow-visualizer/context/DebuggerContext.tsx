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
};

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

    case 'STEP_FORWARD':
      if (!state.trace || state.trace.steps.length === 0) {
        return state;
      }
      // If at end, just pause
      if (state.currentStepIndex >= state.trace.steps.length - 1) {
        return {
          ...state,
          playbackState: 'paused', // Auto-pause at end
        };
      }
      // Move from -1 (ready) to 0, or increment normally
      return {
        ...state,
        currentStepIndex: state.currentStepIndex + 1,
      };

    case 'STEP_BACKWARD':
      // Allow going back to -1 (ready state)
      if (state.currentStepIndex <= -1) return state;
      return {
        ...state,
        currentStepIndex: state.currentStepIndex - 1,
        playbackState: 'paused', // Pause on manual step
      };

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
  isEngineReady: boolean;
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

  const totalSteps = state.trace ? state.trace.steps.length : 0;
  const isAtStart = state.currentStepIndex <= -1; // -1 is "ready" state (before step 0)
  const isAtEnd = state.currentStepIndex >= totalSteps - 1 && state.currentStepIndex >= 0;
  const hasTrace = state.trace !== null && totalSteps > 0;
  const progress = totalSteps > 0 && state.currentStepIndex >= 0
    ? (state.currentStepIndex + 1) / totalSteps
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
    runExecution,
    currentStep,
    currentMessage,
    currentChanges,
    isAtStart,
    isAtEnd,
    hasTrace,
    progress,
    totalSteps,
    isEngineReady,
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
