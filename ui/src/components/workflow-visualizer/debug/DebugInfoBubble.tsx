import { useState, useRef, useEffect } from 'react';
import { createPortal } from 'react-dom';
import { Check, X, Clock, AlertCircle, SkipForward } from 'lucide-react';
import type { ExecutionStep, DebugNodeState } from '../../../types';

interface DebugInfoBubbleProps {
  /** The execution step to display */
  step: ExecutionStep;
  /** Target element to position near */
  targetRef: React.RefObject<HTMLElement>;
  /** Whether the bubble is visible */
  visible: boolean;
  /** Callback when bubble is closed */
  onClose?: () => void;
}

/**
 * Tooltip/bubble showing debug step details
 */
export function DebugInfoBubble({
  step,
  targetRef,
  visible,
  onClose,
}: DebugInfoBubbleProps) {
  const [position, setPosition] = useState({ top: 0, left: 0 });
  const bubbleRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!visible || !targetRef.current) return;

    const updatePosition = () => {
      const targetRect = targetRef.current?.getBoundingClientRect();
      const bubbleRect = bubbleRef.current?.getBoundingClientRect();

      if (!targetRect) return;

      let top = targetRect.bottom + 8;
      let left = targetRect.left;

      // Adjust if bubble would go off screen
      if (bubbleRect) {
        if (left + bubbleRect.width > window.innerWidth - 16) {
          left = window.innerWidth - bubbleRect.width - 16;
        }
        if (top + bubbleRect.height > window.innerHeight - 16) {
          top = targetRect.top - bubbleRect.height - 8;
        }
      }

      setPosition({ top, left });
    };

    updatePosition();
    window.addEventListener('scroll', updatePosition, true);
    window.addEventListener('resize', updatePosition);

    return () => {
      window.removeEventListener('scroll', updatePosition, true);
      window.removeEventListener('resize', updatePosition);
    };
  }, [visible, targetRef]);

  if (!visible) return null;

  const { workflow_id, task_id, result, message } = step;

  // Derive state from result
  const state: DebugNodeState = result === 'executed' ? 'executed' : 'skipped';

  // Check for errors in message
  const hasError = message && message.errors && message.errors.length > 0;
  const displayState: DebugNodeState = hasError ? 'error' : state;

  return createPortal(
    <div
      ref={bubbleRef}
      className="df-debug-bubble"
      style={{
        position: 'fixed',
        top: position.top,
        left: position.left,
      }}
    >
      <div className="df-debug-bubble-header">
        <StateIcon state={displayState} />
        <span className="df-debug-bubble-title">
          {task_id ? 'Task Step' : 'Workflow Skipped'}
        </span>
        {onClose && (
          <button className="df-debug-bubble-close" onClick={onClose}>
            <X size={14} />
          </button>
        )}
      </div>

      <div className="df-debug-bubble-content">
        <div className="df-debug-bubble-row">
          <span className="df-debug-bubble-label">Workflow:</span>
          <span className="df-debug-bubble-value">{workflow_id}</span>
        </div>

        {task_id && (
          <div className="df-debug-bubble-row">
            <span className="df-debug-bubble-label">Task:</span>
            <span className="df-debug-bubble-value">{task_id}</span>
          </div>
        )}

        <div className="df-debug-bubble-row">
          <span className="df-debug-bubble-label">Result:</span>
          <span className={`df-debug-bubble-state df-debug-bubble-state-${displayState}`}>
            {result}
          </span>
        </div>

        {hasError && message?.errors && (
          <div className="df-debug-bubble-error">
            <AlertCircle size={14} />
            <span>{message.errors[0]?.message || 'Unknown error'}</span>
          </div>
        )}
      </div>
    </div>,
    document.body
  );
}

function StateIcon({ state }: { state: DebugNodeState }) {
  switch (state) {
    case 'executed':
      return <Check size={16} className="df-debug-icon-executed" />;
    case 'skipped':
      return <SkipForward size={16} className="df-debug-icon-skipped" />;
    case 'error':
      return <AlertCircle size={16} className="df-debug-icon-error" />;
    case 'current':
      return <Clock size={16} className="df-debug-icon-current" />;
    case 'pending':
    default:
      return <Clock size={16} className="df-debug-icon-pending" />;
  }
}

/**
 * Small badge showing debug state on tree nodes
 */
interface DebugStateBadgeProps {
  state: DebugNodeState;
  conditionResult?: boolean;
  size?: 'sm' | 'md';
}

export function DebugStateBadge({
  state,
  conditionResult,
  size = 'sm',
}: DebugStateBadgeProps) {
  const iconSize = size === 'sm' ? 12 : 14;

  return (
    <span className={`df-debug-badge df-debug-badge-${state} df-debug-badge-${size}`}>
      <StateIcon state={state} />
      {conditionResult !== undefined && (
        <span className={`df-debug-badge-condition ${conditionResult ? 'df-debug-badge-condition-pass' : 'df-debug-badge-condition-fail'}`}>
          {conditionResult ? <Check size={iconSize} /> : <X size={iconSize} />}
        </span>
      )}
    </span>
  );
}
