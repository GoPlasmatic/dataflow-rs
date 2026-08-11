import { Repeat } from 'lucide-react';
import type { LoopConfig } from '../../../types';
import { loopBadgeLabel, loopDescription } from '../../../types';

interface LoopBadgeProps {
  /** The workflow's loop config, or undefined for a one-shot workflow. */
  loop: LoopConfig | undefined;
  className?: string;
}

/**
 * Marks a workflow whose task list repeats. Renders nothing when the workflow
 * has no `loop`, so call sites need no guard of their own.
 */
export function LoopBadge({ loop, className = '' }: LoopBadgeProps) {
  if (!loop) {
    return null;
  }

  return (
    <span className={`df-loop-badge ${className}`} title={loopDescription(loop)}>
      <Repeat size={12} />
      <span className="df-loop-badge-text">{loopBadgeLabel(loop)}</span>
    </span>
  );
}
