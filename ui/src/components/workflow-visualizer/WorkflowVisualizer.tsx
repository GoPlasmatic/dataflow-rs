import { useState, useMemo, useRef } from 'react';
import type { Workflow, Task, TaskGroup, Step, JsonLogicValue, Message, MappingItem, ValidationRule } from '../../types';
import { groupMembers, isTaskGroup, workflowTasks } from '../../types';
import type { DebugConfig } from '../../types/debugConfig';
import { ThemeProvider, useTheme, useDebuggerOptional, DebuggerProvider } from './context';
import { TreeView, DetailsPanel, ResultPanel } from './views';
import { IntegratedDebugToolbar } from './debug';
import { ErrorBoundary } from '../common';
import { useResizable } from './hooks';
import { LAYOUT } from './constants';
import type { Theme } from './context';

import './styles/index.css';
import { describeMappingPath } from '../../utils/dataUtils';

// Extended selection types for tree view
export type TreeSelectionType =
  | { type: 'none' }
  | { type: 'folder'; workflows: Workflow[]; name: string }
  | { type: 'workflow'; workflow: Workflow }
  | { type: 'workflow-condition'; workflow: Workflow; condition: JsonLogicValue }
  | { type: 'task'; task: Task; workflow: Workflow }
  | { type: 'task-condition'; task: Task; workflow: Workflow; condition: JsonLogicValue }
  | { type: 'mapping'; task: Task; workflow: Workflow; mapping: MappingItem; mappingIndex: number }
  | { type: 'validation-rule'; task: Task; workflow: Workflow; rule: ValidationRule; ruleIndex: number };

/**
 * Host-driven selection for {@link WorkflowVisualizer}, by workflow, task or
 * group id.
 */
export type VisualizerSelection =
  | { kind: 'workflow'; workflowId: string }
  | { kind: 'task'; workflowId: string; taskId: string }
  | { kind: 'group'; workflowId: string; groupId: string };

/**
 * Shorthand or explicit initial selection on mount.
 * `"first-workflow"` opens the first workflow's flow view when `workflows` is non-empty.
 */
export type InitialVisualizerSelection = VisualizerSelection | 'first-workflow';

function findGroup(steps: Step[], groupId: string): TaskGroup | undefined {
  for (const step of steps) {
    if (isTaskGroup(step)) {
      if (step.id === groupId) return step;
      const nested = findGroup(groupMembers(step), groupId);
      if (nested) return nested;
    }
  }
  return undefined;
}

function findWorkflowGroup(workflow: Workflow, groupId: string): TaskGroup | undefined {
  return (
    findGroup(workflow.loop?.setup ?? [], groupId) ??
    findGroup(workflow.tasks ?? [], groupId)
  );
}

/**
 * Resolve a host's {@link InitialVisualizerSelection} into the visualizer's
 * internal {@link TreeSelectionType}.
 */
export function resolveVisualizerSelection(
  workflows: Workflow[],
  target: InitialVisualizerSelection | null | undefined,
): TreeSelectionType {
  if (!target) return { type: 'none' };
  if (target === 'first-workflow') {
    if (workflows.length === 0) return { type: 'none' };
    const sorted = [...workflows].sort((a, b) => (a.priority ?? 0) - (b.priority ?? 0));
    return { type: 'workflow', workflow: sorted[0] };
  }
  const workflow = workflows.find((w) => w.id === target.workflowId);
  if (!workflow) return { type: 'none' };
  if (target.kind === 'workflow') {
    return { type: 'workflow', workflow };
  }
  if (target.kind === 'task') {
    const task = workflowTasks(workflow).find((t) => t.id === target.taskId);
    return task ? { type: 'task', task, workflow } : { type: 'none' };
  }
  if (target.kind === 'group') {
    const group = findWorkflowGroup(workflow, target.groupId);
    if (!group) return { type: 'none' };
    const firstTask = workflowTasks({ ...workflow, loop: undefined, tasks: groupMembers(group) })[0];
    return firstTask
      ? { type: 'task', task: firstTask, workflow }
      : { type: 'workflow', workflow };
  }
  return { type: 'none' };
}

/**
 * Convert an internal {@link TreeSelectionType} back to a host-facing
 * {@link VisualizerSelection}, or `null` when nothing (or a folder) is selected.
 */
export function toVisualizerSelection(selection: TreeSelectionType): VisualizerSelection | null {
  switch (selection.type) {
    case 'workflow':
    case 'workflow-condition':
      return { kind: 'workflow', workflowId: selection.workflow.id };
    case 'task':
    case 'task-condition':
    case 'mapping':
    case 'validation-rule':
      return {
        kind: 'task',
        workflowId: selection.workflow.id,
        taskId: selection.task.id,
      };
    default:
      return null;
  }
}

export interface WorkflowVisualizerProps {
  /** Array of workflow definitions to display */
  workflows: Workflow[];
  /** What is selected on mount in uncontrolled mode. Default: nothing. */
  initialSelection?: InitialVisualizerSelection;
  /** Controlled selection; when set (including `null`), the visualizer renders this and reports changes via `onSelectionChange`. */
  selection?: VisualizerSelection | null;
  /** Called whenever the user or debugger changes the selection. */
  onSelectionChange?: (selection: VisualizerSelection | null) => void;
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
  /**
   * Debug configuration for integrated debug mode.
   * When enabled, automatically wraps with DebuggerProvider and shows controls.
   */
  debugConfig?: DebugConfig;
  /**
   * Payload for debugging. Used with debugConfig.
   * Takes precedence over debugConfig.initialPayload.
   */
  debugPayload?: Record<string, unknown>;
}

// Get display info for the current selection
function getSelectionInfo(selection: TreeSelectionType): { title: string; subtitle: string } | null {
  switch (selection.type) {
    case 'folder':
      return {
        title: selection.name,
        subtitle: `${selection.workflows.length} Workflows`,
      };
    case 'workflow':
      return {
        title: selection.workflow.name,
        subtitle: 'Workflow Flow',
      };
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
        title: describeMappingPath(selection.mapping.path),
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

interface VisualizerInnerProps {
  workflows: Workflow[];
  initialSelection?: InitialVisualizerSelection;
  controlledSelection?: VisualizerSelection | null;
  onSelectionChange?: (selection: VisualizerSelection | null) => void;
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  onWorkflowSelect?: (workflow: Workflow) => void;
  executionResult?: Message | null;
  /** Debug config for toolbar and debug mode */
  debugConfig?: DebugConfig;
  /** Payload for debugging */
  debugPayload?: Record<string, unknown>;
}

function VisualizerInner({
  workflows,
  initialSelection,
  controlledSelection,
  onSelectionChange,
  onTaskSelect,
  onWorkflowSelect,
  executionResult,
  debugConfig,
  debugPayload,
}: VisualizerInnerProps) {
  const { resolvedTheme } = useTheme();
  // Derive debug mode from debugConfig
  const debugMode = debugConfig?.enabled ?? false;
  // Use optional hook that returns null if no provider exists
  const debuggerContext = useDebuggerOptional();
  const effectiveDebugContext = debugMode ? debuggerContext : null;
  const [uncontrolledSelection, setUncontrolledSelection] = useState<TreeSelectionType>(() =>
    resolveVisualizerSelection(workflows, initialSelection),
  );
  const selection = useMemo<TreeSelectionType>(() => {
    if (controlledSelection === undefined) return uncontrolledSelection;
    return resolveVisualizerSelection(workflows, controlledSelection);
  }, [controlledSelection, uncontrolledSelection, workflows]);
  const containerRef = useRef<HTMLDivElement>(null);
  const leftPanelRef = useRef<HTMLDivElement>(null);

  const {
    size: leftPanelWidth,
    isDragging,
    onMouseDown: handleMouseDown,
  } = useResizable({
    containerRef,
    direction: 'horizontal',
    min: LAYOUT.LEFT_PANEL.MIN,
    max: LAYOUT.LEFT_PANEL.MAX,
    initial: LAYOUT.LEFT_PANEL.DEFAULT,
  });

  const {
    size: treePanelHeight,
    isDragging: isVerticalDragging,
    onMouseDown: handleVerticalMouseDown,
  } = useResizable({
    containerRef: leftPanelRef,
    direction: 'vertical',
    min: LAYOUT.TREE_HEIGHT_PCT.MIN,
    max: LAYOUT.TREE_HEIGHT_PCT.MAX,
    initial: LAYOUT.TREE_HEIGHT_PCT.DEFAULT,
  });

  const handleSelection = (newSelection: TreeSelectionType) => {
    if (controlledSelection === undefined) {
      setUncontrolledSelection(newSelection);
    }
    onSelectionChange?.(toVisualizerSelection(newSelection));

    // Fire callbacks for external consumers
    if (newSelection.type === 'workflow') {
      onWorkflowSelect?.(newSelection.workflow);
    } else if (newSelection.type === 'workflow-condition') {
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

  const selectionInfo = getSelectionInfo(selection);

  // What the result panel shows: the context at the debugger's step
  // (`trace.initial` before the first), or a host's result message when
  // not debugging. A dataflow-rs 4 trace keeps no message per step.
  const hasDebugTrace = debugMode && effectiveDebugContext?.hasTrace;
  const displayValue = hasDebugTrace
    ? effectiveDebugContext?.currentContext
    : executionResult;
  const currentChanges = hasDebugTrace ? effectiveDebugContext?.currentChanges : [];
  const currentStepIndex = hasDebugTrace ? effectiveDebugContext?.state.currentStepIndex : -1;

  const isDraggingAny = isDragging || isVerticalDragging;

  return (
    <ErrorBoundary>
      {/* `data-theme` is the Signal Board switch, matching the embedded
          datalogic logic editor. The `df-theme-*` class is kept alongside it
          as a deprecated shim for consumers styling against the old pair. */}
      <div
        className={`df-visualizer-container df-theme-${resolvedTheme}`}
        data-theme={resolvedTheme}
      >
        {/* Title bar with debug controls */}
        <div className="df-visualizer-title-bar">
          <span className="df-visualizer-title">Workflows</span>
          {debugMode && debugConfig && (
            <IntegratedDebugToolbar
              workflows={workflows}
              payload={debugPayload ?? debugConfig.initialPayload}
              autoExecute={debugConfig.autoExecute}
              onExecutionComplete={debugConfig.onExecutionComplete}
              onExecutionError={debugConfig.onExecutionError}
            />
          )}
        </div>

        {/* Main content area */}
        <div
          ref={containerRef}
          className={`df-visualizer-horizontal ${isDraggingAny ? 'df-dragging' : ''}`}
        >
          {/* Left section: Tree view + Result panel */}
          <div
            ref={leftPanelRef}
            className="df-visualizer-left"
            style={{ width: leftPanelWidth }}
          >
            {/* Tree view section */}
            <div
              className={`df-visualizer-left-tree ${displayValue ? 'df-with-result' : ''}`}
              style={displayValue ? { height: `${treePanelHeight}%` } : undefined}
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
          {displayValue && (
            <div
              className={`df-visualizer-divider-horizontal ${isVerticalDragging ? 'df-divider-active' : ''}`}
              onMouseDown={handleVerticalMouseDown}
            />
          )}

          {/* Result panel (bottom half when result exists) */}
          {displayValue && (
            <div
              className="df-visualizer-result-panel"
              style={{ height: `${100 - treePanelHeight}%` }}
            >
              <ResultPanel
                displayValue={displayValue}
                currentStepIndex={currentStepIndex ?? -1}
                currentChanges={currentChanges ?? []}
                theme={resolvedTheme === 'dark' ? 'dark' : 'light'}
                hasDebugTrace={!!hasDebugTrace}
              />
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
          <DetailsPanel selection={selection} onSelect={handleSelection} />
        </div>
        </div>
      </div>
    </ErrorBoundary>
  );
}

export function WorkflowVisualizer({
  workflows,
  initialSelection,
  selection,
  onSelectionChange,
  onWorkflowSelect,
  onTaskSelect,
  theme = 'system',
  className = '',
  executionResult,
  debugConfig,
  debugPayload,
}: WorkflowVisualizerProps) {
  // When debugConfig.enabled is true, wrap internally with DebuggerProvider
  if (debugConfig?.enabled) {
    return (
      <ThemeProvider defaultTheme={theme}>
        <DebuggerProvider
          engineFactory={debugConfig.engineFactory}
          autoActivate={true}
          initialPayload={debugPayload ?? debugConfig.initialPayload}
        >
          <div className={`df-root ${className}`}>
            <VisualizerInner
              workflows={workflows}
              initialSelection={initialSelection}
              controlledSelection={selection}
              onSelectionChange={onSelectionChange}
              onTaskSelect={onTaskSelect}
              onWorkflowSelect={onWorkflowSelect}
              executionResult={executionResult}
              debugConfig={debugConfig}
              debugPayload={debugPayload}
            />
          </div>
        </DebuggerProvider>
      </ThemeProvider>
    );
  }

  // Non-debug mode: render without DebuggerProvider
  return (
    <ThemeProvider defaultTheme={theme}>
      <div className={`df-root ${className}`}>
        <VisualizerInner
          workflows={workflows}
          initialSelection={initialSelection}
          controlledSelection={selection}
          onSelectionChange={onSelectionChange}
          onTaskSelect={onTaskSelect}
          onWorkflowSelect={onWorkflowSelect}
          executionResult={executionResult}
        />
      </div>
    </ThemeProvider>
  );
}
