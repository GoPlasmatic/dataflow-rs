import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { GitBranch } from 'lucide-react';
import { branchHandleOffsets } from '../assignBranchSides';

export interface FlowConditionData {
  label: string;
  conditionType: 'workflow' | 'task';
  /** Set by assignBranchSides() from the post-layout geometry. */
  trueOnLeft?: boolean;
  [key: string]: unknown;
}

export const FlowConditionNode = memo(function FlowConditionNode({ data }: NodeProps) {
  const { label, trueOnLeft } = data as FlowConditionData;
  const { trueLeft, falseLeft } = branchHandleOffsets(trueOnLeft);

  return (
    <div className="df-flow-diamond-wrapper">
      <Handle type="target" position={Position.Top} className="df-flow-handle" />
      <div className="df-flow-diamond">
        <div className="df-flow-diamond-content">
          <GitBranch size={14} />
          <span className="df-flow-diamond-label">{label}</span>
        </div>
      </div>
      <Handle
        type="source"
        position={Position.Bottom}
        id="true"
        className="df-flow-handle df-flow-handle-true"
        style={{ left: trueLeft }}
      />
      <Handle
        type="source"
        position={Position.Bottom}
        id="false"
        className="df-flow-handle df-flow-handle-false"
        style={{ left: falseLeft }}
      />
    </div>
  );
});
