import type { Context, JsonLogicValue, MappingItem, ValidationRule } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTaskDebugStep } from '../../hooks/useTreeNodeDebugState';
import { convertMappingsToObject } from '../../../../utils/dataUtils';
import { DataLogicView } from './DataLogicView';

interface TaskContentProps {
  selection: Extract<TreeSelectionType, { type: 'task' }>;
}

export function TaskContent({ selection }: TaskContentProps) {
  const { task } = selection;
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
    return <DataLogicView value={visualData} data={debugData} />;
  }

  // For validation function, show all rules as AND expression
  if (functionName === 'validation') {
    const rules = (input?.rules as ValidationRule[]) || [];
    const andExpression: JsonLogicValue = {
      and: rules.map((rule) => rule.logic),
    };
    return <DataLogicView value={andExpression} data={debugData} />;
  }

  // For custom functions, show the full input
  return <DataLogicView value={task.function.input as JsonLogicValue} />;
}
