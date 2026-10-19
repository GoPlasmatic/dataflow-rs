import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { Context } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme } from '../../context';
import { useTaskDebugStep } from '../../hooks/useTreeNodeDebugState';

interface ValidationRuleContentProps {
  selection: Extract<TreeSelectionType, { type: 'validation-rule' }>;
}

export function ValidationRuleContent({ selection }: ValidationRuleContentProps) {
  const { rule } = selection;
  const { resolvedTheme } = useTheme();
  const shown = useTaskDebugStep(selection.task, selection.workflow);

  // The editor's `data` is what the rule was evaluated against: the context
  // before its task's step, since a validation writes nothing but its
  // records. A skipped task read nothing.
  let debugData: Context | undefined;
  if (shown && shown.step.result !== 'skipped') {
    debugData = shown.contextBefore;
  }

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor">
        <DataLogicEditor
          value={rule.logic}
          theme={resolvedTheme}
          templating={true}
          className="df-datalogic-viewer"
          data={debugData}
        />
      </div>
    </div>
  );
}
