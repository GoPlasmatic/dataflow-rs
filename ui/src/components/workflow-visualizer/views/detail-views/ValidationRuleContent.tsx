import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme, useDebuggerOptional } from '../../context';

interface ValidationRuleContentProps {
  selection: Extract<TreeSelectionType, { type: 'validation-rule' }>;
}

export function ValidationRuleContent({ selection }: ValidationRuleContentProps) {
  const { rule } = selection;
  const { resolvedTheme } = useTheme();
  const dbgContext = useDebuggerOptional();

  // When debugger is active, pass the task-level message context (validation is read-only)
  let debugData: Record<string, unknown> | undefined;
  if (dbgContext?.state.isActive && dbgContext.currentStep) {
    const step = dbgContext.currentStep;
    if (
      step.workflow_id === selection.workflow.id &&
      step.task_id === selection.task.id &&
      step.result === 'executed' &&
      step.message
    ) {
      debugData = step.message.context;
    }
  }

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor">
        <DataLogicEditor
          value={rule.logic}
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
          data={debugData}
        />
      </div>
    </div>
  );
}
