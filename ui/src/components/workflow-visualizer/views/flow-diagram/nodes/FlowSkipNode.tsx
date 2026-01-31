import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { SkipForward } from 'lucide-react';

export const FlowSkipNode = memo(function FlowSkipNode({ data: _data }: NodeProps) {
  return (
    <div className="df-flow-skip-node">
      <Handle type="target" position={Position.Top} className="df-flow-handle" />
      <SkipForward size={14} />
      <span>Skip</span>
      <Handle type="source" position={Position.Bottom} className="df-flow-handle" />
    </div>
  );
});
