import { useState, useRef, useCallback, useEffect } from 'react';
import type { Workflow, Task, JsonLogicValue } from '../../types';
import { ThemeProvider, useTheme } from './context';
import { TreeView, DetailsPanel } from './views';
import { ErrorBoundary } from '../common';
import type { Theme } from './context';

import './styles/visualizer.css';

// Mapping and validation rule types for selection
interface MappingItem {
  path: string;
  logic: JsonLogicValue;
}

interface ValidationRule {
  logic: JsonLogicValue;
  message: string;
}

// Extended selection types for tree view
export type TreeSelectionType =
  | { type: 'none' }
  | { type: 'workflow-condition'; workflow: Workflow; condition: JsonLogicValue }
  | { type: 'task'; task: Task; workflow: Workflow }
  | { type: 'task-condition'; task: Task; workflow: Workflow; condition: JsonLogicValue }
  | { type: 'mapping'; task: Task; workflow: Workflow; mapping: MappingItem; mappingIndex: number }
  | { type: 'validation-rule'; task: Task; workflow: Workflow; rule: ValidationRule; ruleIndex: number };

// Legacy selection type alias for backwards compatibility
export type SelectionType = TreeSelectionType;

export interface WorkflowVisualizerProps {
  /** Array of workflow definitions to display */
  workflows: Workflow[];
  /** Callback when a workflow is selected */
  onWorkflowSelect?: (workflow: Workflow) => void;
  /** Callback when a task is selected */
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  /** Initial theme setting */
  theme?: Theme;
  /** Class name for the root element */
  className?: string;
}

// Get display info for the current selection
function getSelectionInfo(selection: TreeSelectionType): { title: string; subtitle: string } | null {
  switch (selection.type) {
    case 'workflow-condition':
      return {
        title: 'Workflow Condition',
        subtitle: selection.workflow.name,
      };
    case 'task':
      return {
        title: selection.task.name,
        subtitle: `${selection.workflow.name} / ${selection.task.function.name}`,
      };
    case 'task-condition':
      return {
        title: 'Task Condition',
        subtitle: `${selection.workflow.name} / ${selection.task.name}`,
      };
    case 'mapping':
      return {
        title: selection.mapping.path,
        subtitle: `${selection.workflow.name} / ${selection.task.name}`,
      };
    case 'validation-rule':
      return {
        title: selection.rule.message,
        subtitle: `${selection.workflow.name} / ${selection.task.name}`,
      };
    default:
      return null;
  }
}

function WorkflowVisualizerInner({
  workflows,
  onTaskSelect,
  onWorkflowSelect,
}: {
  workflows: Workflow[];
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  onWorkflowSelect?: (workflow: Workflow) => void;
}) {
  const { resolvedTheme } = useTheme();
  const [selection, setSelection] = useState<TreeSelectionType>({ type: 'none' });
  const [leftPanelWidth, setLeftPanelWidth] = useState(280);
  const [isDragging, setIsDragging] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const handleSelection = (newSelection: TreeSelectionType) => {
    setSelection(newSelection);

    // Fire callbacks for external consumers
    if (newSelection.type === 'workflow-condition') {
      onWorkflowSelect?.(newSelection.workflow);
    } else if (
      newSelection.type === 'task' ||
      newSelection.type === 'task-condition' ||
      newSelection.type === 'mapping' ||
      newSelection.type === 'validation-rule'
    ) {
      onTaskSelect?.(newSelection.task, newSelection.workflow);
    }
  };

  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsDragging(true);
  }, []);

  useEffect(() => {
    if (!isDragging) return;

    const handleMouseMove = (e: MouseEvent) => {
      if (containerRef.current) {
        const rect = containerRef.current.getBoundingClientRect();
        const newWidth = e.clientX - rect.left;
        setLeftPanelWidth(Math.max(200, Math.min(450, newWidth)));
      }
    };

    const handleMouseUp = () => {
      setIsDragging(false);
    };

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isDragging]);

  const selectionInfo = getSelectionInfo(selection);

  return (
    <ErrorBoundary>
      <div
        ref={containerRef}
        className={`df-visualizer-horizontal df-theme-${resolvedTheme} ${isDragging ? 'df-dragging' : ''}`}
      >
        {/* Left section: Tree view */}
        <div className="df-visualizer-left" style={{ width: leftPanelWidth }}>
          <div className="df-visualizer-left-header">
            <span className="df-visualizer-left-title">Explorer</span>
          </div>
          <div className="df-visualizer-left-content">
            {workflows.length === 0 ? (
              <div className="df-empty-state">
                <p>No workflows to display</p>
              </div>
            ) : (
              <TreeView
                workflows={workflows}
                selection={selection}
                onSelect={handleSelection}
              />
            )}
          </div>
        </div>

        {/* Resizable divider */}
        <div
          className={`df-visualizer-divider ${isDragging ? 'df-divider-active' : ''}`}
          onMouseDown={handleMouseDown}
        />

        {/* Right section: Details panel */}
        <div className="df-visualizer-right">
          {selectionInfo && (
            <div className="df-details-header">
              <div className="df-details-header-info">
                <span className="df-details-header-title">{selectionInfo.title}</span>
                <span className="df-details-header-subtitle">{selectionInfo.subtitle}</span>
              </div>
            </div>
          )}
          <DetailsPanel selection={selection} />
        </div>
      </div>
    </ErrorBoundary>
  );
}

export function WorkflowVisualizer({
  workflows,
  onWorkflowSelect,
  onTaskSelect,
  theme = 'system',
  className = '',
}: WorkflowVisualizerProps) {
  return (
    <ThemeProvider defaultTheme={theme}>
      <div className={`df-root ${className}`}>
        <WorkflowVisualizerInner
          workflows={workflows}
          onTaskSelect={onTaskSelect}
          onWorkflowSelect={onWorkflowSelect}
        />
      </div>
    </ThemeProvider>
  );
}
