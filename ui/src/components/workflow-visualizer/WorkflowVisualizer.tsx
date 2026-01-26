import { useState, useRef, useCallback, useEffect, useMemo } from 'react';
import type { Workflow, Task, JsonLogicValue, Message, Change } from '../../types';
import { ThemeProvider, useTheme, useDebugger } from './context';
import { TreeView, DetailsPanel } from './views';
import { ErrorBoundary, JsonEditor } from '../common';
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
  /** Execution result to display in the result panel */
  executionResult?: Message | null;
  /** Enable debug mode with step-by-step visualization */
  debugMode?: boolean;
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

function VisualizerInner({
  workflows,
  onTaskSelect,
  onWorkflowSelect,
  executionResult,
  debugMode = false,
}: {
  workflows: Workflow[];
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  onWorkflowSelect?: (workflow: Workflow) => void;
  executionResult?: Message | null;
  debugMode?: boolean;
}) {
  const { resolvedTheme } = useTheme();
  const debuggerContext = debugMode ? useDebugger() : null;
  const [selection, setSelection] = useState<TreeSelectionType>({ type: 'none' });
  const [leftPanelWidth, setLeftPanelWidth] = useState(280);
  const [isDragging, setIsDragging] = useState(false);
  const [treePanelHeight, setTreePanelHeight] = useState(50); // percentage
  const [isVerticalDragging, setIsVerticalDragging] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const leftPanelRef = useRef<HTMLDivElement>(null);

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

  const handleVerticalMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsVerticalDragging(true);
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

  useEffect(() => {
    if (!isVerticalDragging) return;

    const handleMouseMove = (e: MouseEvent) => {
      if (leftPanelRef.current) {
        const rect = leftPanelRef.current.getBoundingClientRect();
        const relativeY = e.clientY - rect.top;
        const percentage = (relativeY / rect.height) * 100;
        setTreePanelHeight(Math.max(20, Math.min(80, percentage)));
      }
    };

    const handleMouseUp = () => {
      setIsVerticalDragging(false);
    };

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);

    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isVerticalDragging]);

  const selectionInfo = getSelectionInfo(selection);

  // Determine which message to display in result panel
  const hasDebugTrace = debugMode && debuggerContext?.hasTrace;
  const displayMessage = hasDebugTrace
    ? debuggerContext?.currentMessage
    : executionResult;
  const currentChanges = hasDebugTrace ? debuggerContext?.currentChanges : [];
  const currentStepIndex = hasDebugTrace ? debuggerContext?.state.currentStepIndex : -1;

  // Compute highlighted paths from changes
  const highlightedPaths = useMemo(() => {
    if (!currentChanges || currentChanges.length === 0) return undefined;
    return currentChanges.map((change: Change) => change.path);
  }, [currentChanges]);

  const isDraggingAny = isDragging || isVerticalDragging;

  return (
    <ErrorBoundary>
      <div
        ref={containerRef}
        className={`df-visualizer-horizontal df-theme-${resolvedTheme} ${isDraggingAny ? 'df-dragging' : ''}`}
      >
        {/* Left section: Tree view + Result panel */}
        <div
          ref={leftPanelRef}
          className="df-visualizer-left"
          style={{ width: leftPanelWidth }}
        >
          {/* Tree view section */}
          <div
            className={`df-visualizer-left-tree ${displayMessage ? 'df-with-result' : ''}`}
            style={displayMessage ? { height: `${treePanelHeight}%` } : undefined}
          >
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
                  debugMode={debugMode}
                />
              )}
            </div>
          </div>

          {/* Horizontal resizer between tree and result */}
          {displayMessage && (
            <div
              className={`df-visualizer-divider-horizontal ${isVerticalDragging ? 'df-divider-active' : ''}`}
              onMouseDown={handleVerticalMouseDown}
            />
          )}

          {/* Result panel (bottom half when result exists) */}
          {displayMessage && (
            <div
              className="df-visualizer-result-panel"
              style={{ height: `${100 - treePanelHeight}%` }}
            >
              <div className="df-visualizer-result-header">
                <span className="df-visualizer-result-title">
                  {hasDebugTrace
                    ? (currentStepIndex !== undefined && currentStepIndex >= 0
                        ? `Step ${currentStepIndex + 1}`
                        : 'Ready')
                    : 'Result'}
                </span>
                {hasDebugTrace && currentChanges && currentChanges.length > 0 && (
                  <span className="df-visualizer-result-changes">
                    {currentChanges.length} change{currentChanges.length !== 1 ? 's' : ''}
                  </span>
                )}
              </div>

              <div className="df-visualizer-result-content">
                <JsonEditor
                  value={JSON.stringify(displayMessage, null, 2)}
                  onChange={() => {}}
                  readOnly={true}
                  theme={resolvedTheme === 'dark' ? 'dark' : 'light'}
                  highlightedPaths={highlightedPaths}
                />
              </div>
            </div>
          )}
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
  executionResult,
  debugMode = false,
}: WorkflowVisualizerProps) {
  return (
    <ThemeProvider defaultTheme={theme}>
      <div className={`df-root ${className}`}>
        <VisualizerInner
          workflows={workflows}
          onTaskSelect={onTaskSelect}
          onWorkflowSelect={onWorkflowSelect}
          executionResult={executionResult}
          debugMode={debugMode}
        />
      </div>
    </ThemeProvider>
  );
}
