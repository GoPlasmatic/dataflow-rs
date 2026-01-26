import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme } from '../../context';

interface ValidationRuleContentProps {
  selection: Extract<TreeSelectionType, { type: 'validation-rule' }>;
}

export function ValidationRuleContent({ selection }: ValidationRuleContentProps) {
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
