import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { GitBranch } from 'lucide-react';

export interface FlowConditionData {
  label: string;
  conditionType: 'workflow' | 'task';
  [key: string]: unknown;
}

export const FlowConditionNode = memo(function FlowConditionNode({ data }: NodeProps) {
  const { label } = data as FlowConditionData;

  return (
    <div className="df-flow-diamond-wrapper">
      <Handle type="target" position={Position.Top} className="df-flow-handle" />
      <div className="df-flow-diamond">
        <div className="df-flow-diamond-content">
          <GitBranch size={14} />
          <span className="df-flow-diamond-label">{label}</span>
        </div>
      </div>
      <Handle type="source" position={Position.Bottom} id="true" className="df-flow-handle df-flow-handle-true" />
      <Handle type="source" position={Position.Bottom} id="false" className="df-flow-handle df-flow-handle-false" />
    </div>
  );
});
