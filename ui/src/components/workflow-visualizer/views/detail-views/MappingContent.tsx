import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { JsonLogicValue } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme } from '../../context';

interface MappingContentProps {
  selection: Extract<TreeSelectionType, { type: 'mapping' }>;
}

export function MappingContent({ selection }: MappingContentProps) {
  const { mapping } = selection;
  const { resolvedTheme } = useTheme();

  // Show the mapping as { path: logic }
  const visualData: Record<string, JsonLogicValue> = {
    [mapping.path]: mapping.logic,
  };

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor">
        <DataLogicEditor
          value={visualData}
          mode="visualize"
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
}
