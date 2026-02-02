import { useState } from 'react';
import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { JsonLogicValue } from '../../../../types';
import { getMappingContext } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme, useDebuggerOptional } from '../../context';

interface MappingContentProps {
  selection: Extract<TreeSelectionType, { type: 'mapping' }>;
}

export function MappingContent({ selection }: MappingContentProps) {
  const { mapping } = selection;
  const { resolvedTheme } = useTheme();
  const dbgContext = useDebuggerOptional();
  const [preserveStructure, setPreserveStructure] = useState(true);

  // Show the mapping as { path: logic }
  const visualData: Record<string, JsonLogicValue> = {
    [mapping.path]: mapping.logic,
  };

  // When debugger is active, find the step matching this task and get the context snapshot
  let debugData: Record<string, unknown> | undefined;
  if (dbgContext?.state.isActive && dbgContext.currentStep) {
    const step = dbgContext.currentStep;
    if (
      step.workflow_id === selection.workflow.id &&
      step.task_id === selection.task.id &&
      step.result === 'executed'
    ) {
      debugData = getMappingContext(step, selection.mappingIndex);
    }
  }

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
