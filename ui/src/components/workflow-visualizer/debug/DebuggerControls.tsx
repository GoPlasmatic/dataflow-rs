import {
  Play,
  Pause,
  Square,
  SkipBack,
  SkipForward,
  RotateCcw,
  Gauge,
} from 'lucide-react';
import { useDebugger } from '../context';
import { useDebuggerKeyboardShortcuts } from '../hooks';

interface DebuggerControlsProps {
  /** Show compact version */
  compact?: boolean;
  /** Additional CSS class */
  className?: string;
}

/**
 * Playback controls for the debugger
 */
export function DebuggerControls({ compact = false, className = '' }: DebuggerControlsProps) {
  const {
    state,
    play,
    pause,
    stop,
    reset,
    stepForward,
    stepBackward,
    setSpeed,
    isAtStart,
    isAtEnd,
    hasTrace,
    progress,
    totalSteps,
    currentFilteredPosition,
  } = useDebugger();

  const { playbackState, playbackSpeed, isExecuting } = state;

  // Keyboard shortcuts
  useDebuggerKeyboardShortcuts({
    playbackState,
    hasTrace,
    isAtStart,
    isAtEnd,
    play,
    pause,
    stepForward,
    stepBackward,
    onStop: stop,
  });

  const handleSpeedChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSpeed(Number(e.target.value));
  };


  if (compact) {
    return (
      <div className={`df-debug-controls df-debug-controls-compact ${className}`}>
        <button
          className="df-debug-btn"
          onClick={stepBackward}
          disabled={!hasTrace || isAtStart || isExecuting}
          title="Previous step (Left Arrow)"
        >
          <SkipBack size={16} />
        </button>

        {playbackState === 'playing' ? (
          <button
            className="df-debug-btn df-debug-btn-primary"
            onClick={pause}
            disabled={!hasTrace || isExecuting}
            title="Pause (Space)"
          >
            <Pause size={18} />
          </button>
        ) : (
          <button
            className="df-debug-btn df-debug-btn-primary"
            onClick={play}
            disabled={!hasTrace || isAtEnd || isExecuting}
            title="Play (Space)"
          >
            <Play size={18} />
          </button>
        )}

        <button
          className="df-debug-btn"
          onClick={stepForward}
          disabled={!hasTrace || isAtEnd || isExecuting}
          title="Next step (Right Arrow)"
        >
          <SkipForward size={16} />
        </button>

        <span className="df-debug-step-counter">
          {Math.max(0, currentFilteredPosition + 1)} / {totalSteps}
        </span>
      </div>
    );
  }

  return (
    <div className={`df-debug-controls ${className}`}>
      {/* Main controls */}
      <div className="df-debug-controls-main">
        <button
          className="df-debug-btn"
          onClick={stop}
          disabled={!hasTrace || isExecuting}
          title="Stop (Escape)"
        >
          <Square size={20} />
        </button>

        <button
          className="df-debug-btn"
          onClick={stepBackward}
          disabled={!hasTrace || isAtStart || isExecuting}
          title="Previous step (Left Arrow)"
        >
          <SkipBack size={20} />
        </button>

        {playbackState === 'playing' ? (
          <button
            className="df-debug-btn df-debug-btn-primary"
            onClick={pause}
            disabled={!hasTrace || isExecuting}
            title="Pause (Space)"
          >
            <Pause size={24} />
          </button>
        ) : (
          <button
            className="df-debug-btn df-debug-btn-primary"
            onClick={play}
            disabled={!hasTrace || isAtEnd || isExecuting}
            title="Play (Space)"
          >
            <Play size={24} />
          </button>
        )}

        <button
          className="df-debug-btn"
          onClick={stepForward}
          disabled={!hasTrace || isAtEnd || isExecuting}
          title="Next step (Right Arrow)"
        >
          <SkipForward size={20} />
        </button>

        <button
          className="df-debug-btn"
          onClick={reset}
          disabled={!hasTrace || isExecuting}
          title="Reset"
        >
          <RotateCcw size={20} />
        </button>
      </div>

      {/* Progress indicator */}
      <div className="df-debug-progress">
        <div className="df-debug-progress-bar">
          <div
            className="df-debug-progress-fill"
            style={{ width: `${progress * 100}%` }}
          />
        </div>
        <span className="df-debug-step-counter">
          Step {Math.max(0, currentFilteredPosition + 1)} of {totalSteps}
        </span>
      </div>

      {/* Speed control */}
      <div className="df-debug-speed">
        <Gauge size={14} />
        <input
          type="range"
          min="100"
          max="2000"
          step="100"
          value={playbackSpeed}
          onChange={handleSpeedChange}
          className="df-debug-speed-slider"
          title={`Playback speed: ${playbackSpeed}ms`}
        />
        <span className="df-debug-speed-label">{playbackSpeed}ms</span>
      </div>
    </div>
  );
}
