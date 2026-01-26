import { Layers } from 'lucide-react';
import '@xyflow/react/dist/style.css';
import '@goplasmatic/datalogic-ui/styles.css';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import {
  DataLogicView,
  TaskContent,
  MappingContent,
  ValidationRuleContent,
} from './detail-views';

interface DetailsPanelProps {
  selection: TreeSelectionType;
}

export function DetailsPanel({ selection }: DetailsPanelProps) {
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

  return (
    <div className="df-details-panel">
      {selection.type === 'workflow-condition' && (
        <DataLogicView value={selection.condition} />
      )}

      {selection.type === 'task-condition' && (
        <DataLogicView value={selection.condition} />
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
