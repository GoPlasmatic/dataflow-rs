import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { JsonLogicValue, MappingItem, ValidationRule } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme } from '../../context';
import { convertMappingsToObject } from '../../../../utils/dataUtils';

interface TaskContentProps {
  selection: Extract<TreeSelectionType, { type: 'task' }>;
}

export function TaskContent({ selection }: TaskContentProps) {
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
            theme={resolvedTheme}
            preserveStructure={true}
            className="df-datalogic-viewer"
          />
        </div>
      </div>
    );
  }

  // For validation function, show all rules as AND expression
  if (functionName === 'validation') {
    const rules = (input?.rules as ValidationRule[]) || [];
    const andExpression: JsonLogicValue = {
      and: rules.map((rule) => rule.logic),
    };
    return (
      <div className="df-details-content">
        <div className="df-details-logic-editor">
          <DataLogicEditor
            value={andExpression}
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
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
}
