import { useState } from 'react';
import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { JsonLogicValue, MappingItem, ValidationRule } from '../../../../types';
import { getMappingContext } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme, useDebuggerOptional } from '../../context';
import { convertMappingsToObject } from '../../../../utils/dataUtils';

interface TaskContentProps {
  selection: Extract<TreeSelectionType, { type: 'task' }>;
}

export function TaskContent({ selection }: TaskContentProps) {
  const { task } = selection;
  const { resolvedTheme } = useTheme();
  const dbgContext = useDebuggerOptional();
  const [preserveStructure, setPreserveStructure] = useState(true);
  const functionName = task.function.name;
  const input = task.function.input as Record<string, unknown> | undefined;

  // Compute debug data context when debugger is active and step matches
  let debugData: Record<string, unknown> | undefined;
  if (dbgContext?.state.isActive && dbgContext.currentStep) {
    const step = dbgContext.currentStep;
    if (
      step.workflow_id === selection.workflow.id &&
      step.task_id === task.id &&
      step.result === 'executed'
    ) {
      if (functionName === 'map') {
        // For aggregate map view, use context before first mapping
        debugData = getMappingContext(step, 0);
      } else if (functionName === 'validation' && step.message) {
        // Validation is read-only, use task-level context
        debugData = step.message.context;
      }
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
            preserveStructure={preserveStructure}
            onPreserveStructureChange={setPreserveStructure}
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
            preserveStructure={preserveStructure}
            onPreserveStructureChange={setPreserveStructure}
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
          preserveStructure={preserveStructure}
          onPreserveStructureChange={setPreserveStructure}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
}
