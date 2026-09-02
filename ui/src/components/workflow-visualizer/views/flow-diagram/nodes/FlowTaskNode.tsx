import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { AlertTriangle, CircleStop, ShieldX } from 'lucide-react';
import { FunctionTypeBadge } from '../../../cards/FunctionTypeBadge';

export interface FlowTaskData {
  taskName: string;
  functionName: string;
  description?: string;
  continueOnError?: boolean;
  terminal?: boolean;
  haltOn?: 'never' | 'failure';
  [key: string]: unknown;
}

export const FlowTaskNode = memo(function FlowTaskNode({ data }: NodeProps) {
  const { taskName, functionName, description, continueOnError, terminal, haltOn } =
    data as FlowTaskData;

  return (
    <div className="df-flow-task-node">
      <Handle type="target" position={Position.Top} className="df-flow-handle" />
      <div className="df-flow-task-node-header">
        <span className="df-flow-task-node-name">{taskName}</span>
        {continueOnError && (
          <span className="df-flow-task-node-coe" title="Continue on error">
            <AlertTriangle size={12} />
          </span>
        )}
        {terminal && (
          <span className="df-flow-task-node-terminal" title="Terminal — ends the workflow">
            <CircleStop size={12} />
          </span>
        )}
        {haltOn === 'failure' && (
          <span
            className="df-flow-task-node-halt-on"
            title="halt_on: failure — ends the workflow when this task fails"
          >
            <ShieldX size={12} />
          </span>
        )}
      </div>
      <div className="df-flow-task-node-badge">
        <FunctionTypeBadge functionName={functionName} />
      </div>
      {description && (
        <div className="df-flow-task-node-desc">{description}</div>
      )}
      <Handle type="source" position={Position.Bottom} className="df-flow-handle" />
    </div>
  );
});
