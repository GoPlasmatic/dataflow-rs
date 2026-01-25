import { Layers } from 'lucide-react';
import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import '@xyflow/react/dist/style.css';
import '@goplasmatic/datalogic-ui/styles.css';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import type { JsonLogicValue } from '../../../types';
import { useTheme } from '../context';

interface DetailsPanelProps {
  selection: TreeSelectionType;
}

interface MappingItem {
  path: string;
  logic: JsonLogicValue;
}

interface ValidationRule {
  logic: JsonLogicValue;
  message: string;
}

// Convert mappings array to object notation for DataLogic visualization
function convertMappingsToObject(mappings: MappingItem[]): Record<string, JsonLogicValue> {
  const result: Record<string, JsonLogicValue> = {};
  for (const mapping of mappings) {
    result[mapping.path] = mapping.logic;
  }
  return result;
}

function DataLogicView({ value }: { value: JsonLogicValue }) {
  const { resolvedTheme } = useTheme();

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor">
        <DataLogicEditor
          value={value}
          mode="visualize"
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
}

function TaskContent({ selection }: { selection: Extract<TreeSelectionType, { type: 'task' }> }) {
  const { task } = selection;
  const { resolvedTheme } = useTheme();
  const functionName = task.function.name;
  const input = task.function.input as Record<string, unknown> | undefined;

  // For map function, show all mappings
  if (functionName === 'map') {
    const mappings = (input?.mappings as MappingItem[]) || [];
    const visualData = convertMappingsToObject(mappings);
    return (
      <div className="df-details-content">
        <div className="df-details-logic-editor">
          <DataLogicEditor
            value={visualData}
            mode="visualize"
            theme={resolvedTheme}
            preserveStructure={true}
            className="df-datalogic-viewer"
          />
        </div>
      </div>
    );
  }

  // For validation function, show all rules
  if (functionName === 'validation') {
    const rules = (input?.rules as ValidationRule[]) || [];
    const rulesVisualization: Record<string, JsonLogicValue> = {};
    rules.forEach((rule, index) => {
      rulesVisualization[`Rule #${index + 1}: ${rule.message}`] = rule.logic;
    });
    return (
      <div className="df-details-content">
        <div className="df-details-logic-editor">
          <DataLogicEditor
            value={rulesVisualization}
            mode="visualize"
            theme={resolvedTheme}
            preserveStructure={true}
            className="df-datalogic-viewer"
          />
        </div>
      </div>
    );
  }

  // For custom functions, show the full input
  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor">
        <DataLogicEditor
          value={task.function.input as JsonLogicValue}
          mode="visualize"
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
}

function MappingContent({ selection }: { selection: Extract<TreeSelectionType, { type: 'mapping' }> }) {
  const { mapping } = selection;
  const { resolvedTheme } = useTheme();

  // Show the mapping as { path: logic }
  const visualData: Record<string, JsonLogicValue> = {
    [mapping.path]: mapping.logic,
  };

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor">
        <DataLogicEditor
          value={visualData}
          mode="visualize"
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
}

function ValidationRuleContent({ selection }: { selection: Extract<TreeSelectionType, { type: 'validation-rule' }> }) {
  const { rule } = selection;
  const { resolvedTheme } = useTheme();

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor">
        <DataLogicEditor
          value={rule.logic}
          mode="visualize"
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
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
