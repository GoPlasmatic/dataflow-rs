import { memo } from 'react';
import { Handle, Position, type NodeProps } from '@xyflow/react';
import { Repeat } from 'lucide-react';

export interface FlowLoopData {
  /** Text inside the node: `i < 10000` for a guard, `i += 1` for a tail. */
  label: string;
  /** `guard` renders the bound-check diamond, `tail` the counter advance. */
  variant: 'guard' | 'tail';
  [key: string]: unknown;
}

/**
 * The two engine-owned ends of a workflow loop.
 *
 * `guard` is the `counter < max` bound check the engine performs before each
 * sweep; its false branch is a loop exit. `tail` is the counter advance that
 * follows the last task. The back-edge runs tail -> guard, entering and
 * leaving on the left so it sweeps clear of the main column.
 */
export const FlowLoopNode = memo(function FlowLoopNode({ data }: NodeProps) {
  const { label, variant } = data as FlowLoopData;

  if (variant === 'guard') {
    return (
      <div className="df-flow-diamond-wrapper">
        <Handle type="target" position={Position.Top} className="df-flow-handle" />
        <Handle
          type="target"
          id="loop-in"
          position={Position.Left}
          className="df-flow-handle df-flow-handle-loop"
        />
        <div className="df-flow-diamond df-flow-loop-diamond">
          <div className="df-flow-diamond-content">
            <Repeat size={14} />
            <span className="df-flow-diamond-label">{label}</span>
          </div>
        </div>
        <Handle
          type="source"
          position={Position.Bottom}
          id="true"
          className="df-flow-handle df-flow-handle-true"
        />
        <Handle
          type="source"
          position={Position.Bottom}
          id="false"
          className="df-flow-handle df-flow-handle-false"
        />
      </div>
    );
  }

  return (
    <div className="df-flow-pill df-flow-pill-loop">
      <Handle type="target" position={Position.Top} className="df-flow-handle" />
      <div className="df-flow-pill-content">
        <Repeat size={14} />
        <span>{label}</span>
      </div>
      <Handle
        type="source"
        id="loop-out"
        position={Position.Left}
        className="df-flow-handle df-flow-handle-loop"
      />
    </div>
  );
});
