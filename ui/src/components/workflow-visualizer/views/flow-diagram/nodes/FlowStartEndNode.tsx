import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { Play, CheckCircle } from 'lucide-react';

export interface FlowStartEndData {
  label: string;
  variant: 'start' | 'end';
  [key: string]: unknown;
}

export const FlowStartEndNode = memo(function FlowStartEndNode({ data }: NodeProps) {
  const { label, variant } = data as FlowStartEndData;
  const isStart = variant === 'start';

  return (
    <div className={`df-flow-pill df-flow-pill-${variant}`}>
      {!isStart && <Handle type="target" position={Position.Top} className="df-flow-handle" />}
      <div className="df-flow-pill-content">
        {isStart ? <Play size={14} /> : <CheckCircle size={14} />}
        <span>{label}</span>
      </div>
      {isStart && <Handle type="source" position={Position.Bottom} className="df-flow-handle" />}
    </div>
  );
});
