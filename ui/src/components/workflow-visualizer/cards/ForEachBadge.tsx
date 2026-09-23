import { Split } from 'lucide-react';
import type { ForEach } from '../../../types';
import { forEachBadgeLabel, forEachDescription } from '../../../types';

interface ForEachBadgeProps {
  /** The task's fan-out, or undefined for a task that runs once. */
  forEach: ForEach | undefined;
  className?: string;
}

/**
 * Marks a task that runs once per element of an array. Renders nothing when
 * the task has no `for_each`, so call sites need no guard of their own.
 * Shares the loop badge's styling: both say "this repeats".
 */
export function ForEachBadge({ forEach, className = '' }: ForEachBadgeProps) {
  if (!forEach) {
    return null;
  }

  return (
    <span className={`df-loop-badge ${className}`} title={forEachDescription(forEach)}>
      <Split size={12} />
      <span className="df-loop-badge-text">{forEachBadgeLabel(forEach)}</span>
    </span>
  );
}
