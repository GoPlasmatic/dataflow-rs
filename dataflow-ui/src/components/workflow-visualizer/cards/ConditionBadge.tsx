import { useState, useRef, useEffect } from 'react';
import { createPortal } from 'react-dom';
import { Check, GitBranch } from 'lucide-react';
import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import '@goplasmatic/datalogic-ui/styles.css';
import type { JsonLogicValue } from '../../../types';

interface ConditionBadgeProps {
  condition: JsonLogicValue | undefined;
  className?: string;
  onClick?: () => void;
}

function formatConditionPreview(condition: JsonLogicValue | undefined): string {
  if (condition === undefined || condition === null) {
    return 'always';
  }

  if (condition === true) {
    return 'always';
  }

  if (condition === false) {
    return 'never';
  }

  // Try to create a human-readable preview
  if (typeof condition === 'object' && !Array.isArray(condition)) {
    const keys = Object.keys(condition);
    if (keys.length === 1) {
      const operator = keys[0];
      const args = condition[operator];

      // Handle common comparison operators
      if (['==', '===', '!=', '!==', '>', '>=', '<', '<='].includes(operator)) {
        if (Array.isArray(args) && args.length >= 2) {
          const left = formatArg(args[0]);
          const right = formatArg(args[1]);
          return `${left} ${operator} ${right}`;
        }
      }

      // Handle logical operators
      if (operator === 'and' || operator === 'or') {
        if (Array.isArray(args)) {
          return `${args.length} conditions (${operator})`;
        }
      }

      // Handle var
      if (operator === 'var') {
        return `${args}`;
      }
    }
  }

  // Fallback: stringify and truncate
  const str = JSON.stringify(condition);
  if (str.length > 40) {
    return str.slice(0, 37) + '...';
  }
  return str;
}

function formatArg(arg: JsonLogicValue): string {
  if (typeof arg === 'object' && arg !== null && !Array.isArray(arg)) {
    const keys = Object.keys(arg);
    if (keys.length === 1 && keys[0] === 'var') {
      return String(arg['var']);
    }
  }
  if (typeof arg === 'string') {
    return `"${arg}"`;
  }
  return JSON.stringify(arg);
}


interface TooltipPosition {
  top: number;
  left: number;
  showBelow: boolean;
}

export function ConditionBadge({ condition, className = '', onClick }: ConditionBadgeProps) {
  const [showTooltip, setShowTooltip] = useState(false);
  const [tooltipPosition, setTooltipPosition] = useState<TooltipPosition>({ top: 0, left: 0, showBelow: false });
  const badgeRef = useRef<HTMLSpanElement>(null);
  const isAlways = condition === undefined || condition === null || condition === true;
  const preview = formatConditionPreview(condition);

  useEffect(() => {
    if (showTooltip && badgeRef.current) {
      const rect = badgeRef.current.getBoundingClientRect();
      const tooltipWidth = 360;
      const tooltipHeight = 200; // Estimated max height

      // Position above the badge by default
      let top = rect.top - 8;
      let left = rect.left;
      let showBelow = false;

      // Check if tooltip would go off the right edge
      if (left + tooltipWidth > window.innerWidth - 16) {
        left = window.innerWidth - tooltipWidth - 16;
      }

      // Check if tooltip would go off the left edge
      if (left < 16) {
        left = 16;
      }

      // Check if tooltip would go off the top edge, if so show below
      if (top - tooltipHeight < 16) {
        top = rect.bottom + 8;
        showBelow = true;
      }

      setTooltipPosition({ top, left, showBelow });
    }
  }, [showTooltip]);

  return (
    <span
      ref={badgeRef}
      className={`df-condition-badge ${isAlways ? 'df-condition-always' : 'df-condition-conditional'} ${className}`}
      onClick={onClick}
      onMouseEnter={() => setShowTooltip(true)}
      onMouseLeave={() => setShowTooltip(false)}
      role={onClick ? 'button' : undefined}
      tabIndex={onClick ? 0 : undefined}
    >
      {isAlways ? (
        <>
          <Check size={12} />
          <span>always</span>
        </>
      ) : (
        <>
          <GitBranch size={12} />
          <span>{preview}</span>
        </>
      )}
      {showTooltip && !isAlways && createPortal(
        <div
          className="df-condition-tooltip"
          style={{
            position: 'fixed',
            top: tooltipPosition.top,
            left: tooltipPosition.left,
            transform: tooltipPosition.showBelow ? 'none' : 'translateY(-100%)',
          }}
        >
          <div className="df-condition-tooltip-header">Logic</div>
          <div className="df-condition-tooltip-content">
            <DataLogicEditor
              value={condition}
              mode="visualize"
              theme="dark"
              preserveStructure={true}
              className="df-condition-tooltip-editor"
            />
          </div>
        </div>,
        document.body
      )}
    </span>
  );
}
