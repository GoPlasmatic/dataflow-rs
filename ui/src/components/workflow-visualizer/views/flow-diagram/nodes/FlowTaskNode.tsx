import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { AlertTriangle, CircleStop, ShieldX } from 'lucide-react';
import { FunctionTypeBadge } from '../../../cards/FunctionTypeBadge';
import { ForEachBadge } from '../../../cards/ForEachBadge';
import type { ForEach } from '../../../../../types';

export interface FlowTaskData {
  taskName: string;
  functionName: string;
  description?: string;
  /** `on_failure: "continue"` in either spelling (see `taskContinuesOnFailure`). */
  continuesOnFailure?: boolean;
  terminal?: boolean;
  /** A halting policy in either spelling (see `taskHaltsOnFailure`). */
  haltsOnFailure?: boolean;
  /** The task's fan-out, when it runs once per element. */
  forEach?: ForEach;
  [key: string]: unknown;
}

export const FlowTaskNode = memo(function FlowTaskNode({ data }: NodeProps) {
  const {
    taskName,
    functionName,
    description,
    continuesOnFailure,
    terminal,
    haltsOnFailure,
    forEach,
  } = data as FlowTaskData;

  return (
    <div className="df-flow-task-node">
      <Handle type="target" position={Position.Top} className="df-flow-handle" />
      <div className="df-flow-task-node-header">
        <span className="df-flow-task-node-name">{taskName}</span>
        {continuesOnFailure && (
          <span
            className="df-flow-task-node-coe"
            title="on_failure: continue — a failure is recorded and the next task runs"
          >
            <AlertTriangle size={12} />
          </span>
        )}
        {terminal && (
          <span className="df-flow-task-node-terminal" title="Terminal — ends the workflow">
            <CircleStop size={12} />
          </span>
        )}
        {haltsOnFailure && (
          <span
            className="df-flow-task-node-halt-on"
            title="Halts the workflow on failure (on_rejection: halt, or on_failure: halt)"
          >
            <ShieldX size={12} />
          </span>
        )}
      </div>
      <div className="df-flow-task-node-badge">
        <FunctionTypeBadge functionName={functionName} />
        <ForEachBadge forEach={forEach} className="df-flow-task-node-for-each" />
      </div>
      {description && (
        <div className="df-flow-task-node-desc">{description}</div>
      )}
      <Handle type="source" position={Position.Bottom} className="df-flow-handle" />
    </div>
  );
});
