import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { AlertTriangle } from 'lucide-react';
import { FunctionTypeBadge } from '../../../cards/FunctionTypeBadge';

export interface FlowTaskData {
  taskName: string;
  functionName: string;
  description?: string;
  continueOnError?: boolean;
  [key: string]: unknown;
}

export const FlowTaskNode = memo(function FlowTaskNode({ data }: NodeProps) {
  const { taskName, functionName, description, continueOnError } = data as FlowTaskData;

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
