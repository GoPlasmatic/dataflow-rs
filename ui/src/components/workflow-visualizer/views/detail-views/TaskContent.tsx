import { useMemo } from 'react';
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
  // the first mapping's (the mapping view shows each its own); validation,
  // filter, log, and custom functions read the context before the step. A
  // skipped task read nothing.
  const debugData: Context | undefined =
    shown && shown.step.result !== 'skipped' ? shown.contextBefore : undefined;

  const value = useMemo<JsonLogicValue | Record<string, JsonLogicValue>>(() => {
    if (functionName === 'map') {
      const mappings = (input?.mappings as MappingItem[]) || [];
      return convertMappingsToObject(mappings);
    }
    if (functionName === 'validation') {
      const rules = (input?.rules as ValidationRule[]) || [];
      return {
        and: rules.map((rule) => rule.logic),
      };
    }
    return task.function.input as JsonLogicValue;
  }, [functionName, input, task.function.input]);

  return <DataLogicView value={value} data={debugData} />;
}
