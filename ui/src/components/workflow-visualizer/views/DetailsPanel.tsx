import { Layers } from 'lucide-react';
import '@xyflow/react/dist/style.css';
import '@goplasmatic/datalogic-ui/styles.css';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import { useDebuggerOptional } from '../context';
import { getMessageAtStep } from '../../../types';
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

  // Compute condition debug data: use the message from the step before the condition was evaluated
  let conditionDebugData: Record<string, unknown> | undefined;
  if (dbgContext?.state.isActive && dbgContext.state.trace && dbgContext.state.currentStepIndex >= 0) {
    const { trace, currentStepIndex } = dbgContext.state;

    if (selection.type === 'workflow-condition' || selection.type === 'task-condition') {
      // For conditions, the context used for evaluation is from the previous step's message
      // (the state just before the condition was checked)
      const prevMessage = currentStepIndex > 0
        ? getMessageAtStep(trace, currentStepIndex - 1)
        : null;
      if (prevMessage) {
        conditionDebugData = prevMessage.context;
      }
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
