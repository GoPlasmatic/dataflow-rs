import { Check, X, AlertCircle, Clock, SkipForward } from 'lucide-react';
import type { DebugNodeState } from '../../../types';

interface DebugStateIconProps {
  state: DebugNodeState;
  conditionResult?: boolean;
}

export function DebugStateIcon({ state, conditionResult }: DebugStateIconProps) {
  // For condition nodes, show pass/fail indicator
  if (conditionResult !== undefined) {
    return conditionResult ? (
      <Check size={12} className="df-debug-icon-pass" />
    ) : (
      <X size={12} className="df-debug-icon-fail" />
    );
  }

  switch (state) {
    case 'executed':
      return <Check size={12} className="df-debug-icon-executed" />;
    case 'skipped':
      return <SkipForward size={12} className="df-debug-icon-skipped" />;
    case 'error':
      return <AlertCircle size={12} className="df-debug-icon-error" />;
    case 'current':
      return <Clock size={12} className="df-debug-icon-current" />;
    case 'pending':
      return <Clock size={12} className="df-debug-icon-pending" />;
    default:
      return null;
  }
}
