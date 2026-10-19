import type { Context } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTaskDebugStep } from '../../hooks/useTreeNodeDebugState';
import { DataLogicView } from './DataLogicView';

interface ValidationRuleContentProps {
  selection: Extract<TreeSelectionType, { type: 'validation-rule' }>;
}

export function ValidationRuleContent({ selection }: ValidationRuleContentProps) {
  const { rule } = selection;
  const shown = useTaskDebugStep(selection.task, selection.workflow);

  // The editor's `data` is what the rule was evaluated against: the context
  // before its task's step, since a validation writes nothing but its
  // records. A skipped task read nothing.
  let debugData: Context | undefined;
  if (shown && shown.step.result !== 'skipped') {
    debugData = shown.contextBefore;
  }

  return <DataLogicView value={rule.logic} data={debugData} />;
}
