import { ChevronDown, ChevronRight } from 'lucide-react';
import type { DebugNodeState } from '../../../types';
import { DebugStateIcon } from './DebugStateIcon';

export interface TreeNodeProps {
  label: string;
  icon?: React.ReactNode;
  iconColor?: string;
  isExpanded?: boolean;
  isSelected?: boolean;
  hasChildren?: boolean;
  level: number;
  onToggle?: () => void;
  onClick?: () => void;
  children?: React.ReactNode;
  /** Debug state for this node */
  debugState?: DebugNodeState | null;
  /** Condition result (for condition nodes) */
  conditionResult?: boolean;
  /** Whether this is the current step */
  isCurrent?: boolean;
}

export function TreeNode({
  label,
  icon,
  iconColor,
  isExpanded = false,
  isSelected = false,
  hasChildren = false,
  level,
  onToggle,
  onClick,
  children,
  debugState,
  conditionResult,
  isCurrent,
}: TreeNodeProps) {
  const handleClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onClick?.();
  };

  const handleToggle = (e: React.MouseEvent) => {
    e.stopPropagation();
    onToggle?.();
  };

  const debugStateClass = debugState ? `df-tree-node-${debugState}` : '';
  const currentClass = isCurrent ? 'df-tree-node-current-step' : '';

  return (
    <div
      className={`df-tree-node ${debugStateClass} ${currentClass}`}
      data-current-step={isCurrent ? 'true' : undefined}
    >
      <div
        className={`df-tree-node-content ${isSelected ? 'df-tree-node-selected' : ''}`}
        style={{ paddingLeft: `${level * 16 + 8}px` }}
        onClick={handleClick}
      >
        <span
          className="df-tree-toggle"
          onClick={hasChildren ? handleToggle : undefined}
          style={{ visibility: hasChildren ? 'visible' : 'hidden' }}
        >
          {isExpanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </span>
        {icon && <span className="df-tree-icon" style={iconColor ? { color: iconColor } : undefined}>{icon}</span>}
        <span className="df-tree-label">{label}</span>

        {/* Debug state indicator */}
        {debugState && (
          <span className="df-tree-debug-indicator">
            <DebugStateIcon state={debugState} conditionResult={conditionResult} />
          </span>
        )}
      </div>
      {isExpanded && hasChildren && (
        <div className="df-tree-children">{children}</div>
      )}
    </div>
  );
}
