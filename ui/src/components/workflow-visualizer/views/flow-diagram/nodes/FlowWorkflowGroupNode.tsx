import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { Layers, GitBranch, Check } from 'lucide-react';
import type { LoopConfig } from '../../../../../types';
import { LoopBadge } from '../../../cards';

export interface FlowWorkflowGroupData {
  workflowName: string;
  priority?: number;
  taskCount: number;
  hasCondition: boolean;
  workflowId: string;
  loop?: LoopConfig;
  [key: string]: unknown;
}

export const FlowWorkflowGroupNode = memo(function FlowWorkflowGroupNode({ data }: NodeProps) {
  const { workflowName, priority, taskCount, hasCondition, loop } = data as FlowWorkflowGroupData;

  return (
    <div className="df-flow-wf-group-node">
      <Handle type="target" position={Position.Top} className="df-flow-handle" />
      <div className="df-flow-wf-group-node-header">
        <Layers size={14} />
        <span className="df-flow-wf-group-node-name">{workflowName}</span>
        {priority !== undefined && (
          <span className="df-flow-wf-group-node-priority">P{priority}</span>
        )}
      </div>
      <div className="df-flow-wf-group-node-meta">
        <span className="df-flow-wf-group-node-meta-item">
          {taskCount} {taskCount === 1 ? 'task' : 'tasks'}
        </span>
        <span className={`df-flow-wf-group-node-meta-item ${hasCondition ? 'df-flow-wf-group-node-condition' : 'df-flow-wf-group-node-always'}`}>
          {hasCondition ? <GitBranch size={12} /> : <Check size={12} />}
          {hasCondition ? 'Conditional' : 'Always'}
        </span>
        <LoopBadge loop={loop} />
      </div>
      <Handle type="source" position={Position.Bottom} className="df-flow-handle" />
    </div>
  );
});
