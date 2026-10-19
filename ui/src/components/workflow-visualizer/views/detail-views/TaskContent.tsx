import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { Context, JsonLogicValue, MappingItem, ValidationRule } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme } from '../../context';
import { useTaskDebugStep } from '../../hooks/useTreeNodeDebugState';
import { convertMappingsToObject } from '../../../../utils/dataUtils';

interface TaskContentProps {
  selection: Extract<TreeSelectionType, { type: 'task' }>;
}

export function TaskContent({ selection }: TaskContentProps) {
  const { task } = selection;
  const { resolvedTheme } = useTheme();
  const shown = useTaskDebugStep(task, selection.workflow);
  const functionName = task.function.name;
  const input = task.function.input as Record<string, unknown> | undefined;

  // The editor's `data` is what the logic was evaluated against: the context
  // before the task's step. A map's mappings each saw the one before them
  // write, which a single object cannot show, so the aggregate view takes
  // the first mapping's (the mapping view shows each its own); a validation
  // writes nothing but its records, so every rule read the same one. A
  // skipped task read nothing.
  let debugData: Context | undefined;
  if (shown && shown.step.result !== 'skipped') {
    if (functionName === 'map' || functionName === 'validation') {
      debugData = shown.contextBefore;
    }
  }

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
            templating={true}
            className="df-datalogic-viewer"
            data={debugData}
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
            templating={true}
            className="df-datalogic-viewer"
            data={debugData}
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
          templating={true}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
}
