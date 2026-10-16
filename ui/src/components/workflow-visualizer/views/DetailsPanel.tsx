import { Layers } from 'lucide-react';
import '@xyflow/react/dist/style.css';
import '@goplasmatic/datalogic-ui/styles.css';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import { useDebuggerOptional } from '../context';
import type { Context, Trace } from '../../../types';
import { contextAtStep, findTaskStepIndex, isEngineStep } from '../../../types';
import {
  DataLogicView,
  TaskContent,
  MappingContent,
  ValidationRuleContent,
} from './detail-views';
import { WorkflowFlowDiagram, GroupFlowDiagram } from './flow-diagram';

interface DetailsPanelProps {
  selection: TreeSelectionType;
  onSelect?: (selection: TreeSelectionType) => void;
}

/**
 * The index of the step before which a workflow's condition was evaluated,
 * at the current position, or -1 when the workflow has no step at or before
 * it.
 *
 * A trace records no evaluation of its own, so it is placed: the condition
 * ran just before the workflow's first step, and, in a loop, again after the
 * engine's writes as each sweep is entered (its counter, `as` and
 * `scratch`), before the sweep's tasks. From the workflow's latest step at or
 * before the position, walk back through its steps to that point.
 */
function workflowConditionStepIndex(trace: Trace, currentStepIndex: number, workflowId: string): number {
  let i = -1;
  for (let k = Math.min(currentStepIndex, trace.steps.length - 1); k >= 0; k--) {
    if (trace.steps[k].workflow_id === workflowId) {
      i = k;
      break;
    }
  }
  if (i === -1) return -1;
  for (;;) {
    // A sweep entered: the condition read what the engine's writes left.
    if (isEngineStep(trace.steps[i])) return i + 1;
    // The workflow's first step: the condition read what came before it.
    if (i === 0 || trace.steps[i - 1].workflow_id !== workflowId) return i;
    i--;
  }
}

export function DetailsPanel({ selection, onSelect }: DetailsPanelProps) {
  const dbgContext = useDebuggerOptional();

  if (selection.type === 'none') {
    return (
      <div className="df-details-panel df-details-empty">
        <div className="df-details-empty-content">
          <Layers size={40} className="df-details-empty-icon" />
          <p>Select an item from the explorer to view its details and logic visualization</p>
        </div>
      </div>
    );
  }

  if (selection.type === 'folder' && onSelect) {
    return (
      <div className="df-details-panel">
        <GroupFlowDiagram
          workflows={selection.workflows}
          name={selection.name}
          onSelect={onSelect}
        />
      </div>
    );
  }

  if (selection.type === 'workflow' && onSelect) {
    return (
      <div className="df-details-panel">
        <WorkflowFlowDiagram workflow={selection.workflow} onSelect={onSelect} />
      </div>
    );
  }

  // A condition's data is the context before the step it gated, at the
  // current position: for a task, the task's latest step at or before it
  // (the current sweep's, for a loop; a skipped step's condition read the
  // context before it too); for a workflow, see above. A step after the
  // position shows nothing, as the tree marks it pending. Nor does a
  // workflow its rollout refused: its `skipped` step looks like the
  // condition's, but the condition was never read, and only the run's
  // record (`selection`) says so.
  let conditionDebugData: Context | undefined;
  if (dbgContext?.state.isActive && dbgContext.state.trace && dbgContext.state.currentStepIndex >= 0) {
    const { trace, run, contexts, currentStepIndex } = dbgContext.state;

    let stepIndex = -1;
    if (selection.type === 'task-condition') {
      stepIndex = findTaskStepIndex(trace, currentStepIndex, selection.workflow.id, selection.task.id);
      if (stepIndex > currentStepIndex) stepIndex = -1;
    } else if (selection.type === 'workflow-condition') {
      const workflowId = selection.workflow.id;
      const record = run?.workflows.find((w) => w.id === workflowId);
      if (record?.selection !== 'skipped_by_rollout') {
        stepIndex = workflowConditionStepIndex(trace, currentStepIndex, workflowId);
      }
    }
    if (stepIndex !== -1) {
      conditionDebugData = contextAtStep(trace, contexts, stepIndex - 1);
    }
  }

  return (
    <div className="df-details-panel">
      {selection.type === 'workflow-condition' && (
        <DataLogicView value={selection.condition} data={conditionDebugData} />
      )}

      {selection.type === 'task-condition' && (
        <DataLogicView value={selection.condition} data={conditionDebugData} />
      )}

      {selection.type === 'task' && (
        <TaskContent selection={selection} />
      )}

      {selection.type === 'mapping' && (
        <MappingContent selection={selection} />
      )}

      {selection.type === 'validation-rule' && (
        <ValidationRuleContent selection={selection} />
      )}
    </div>
  );
}
