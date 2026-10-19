import { useMemo } from 'react';
import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { Context, JsonLogicValue } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme } from '../../context';
import { useTaskDebugStep } from '../../hooks/useTreeNodeDebugState';
import { describeMappingPath, describeMappingValue } from '../../../../utils/dataUtils';

interface MappingContentProps {
  selection: Extract<TreeSelectionType, { type: 'mapping' }>;
}

export function MappingContent({ selection }: MappingContentProps) {
  const { mapping, mappingIndex } = selection;
  const { resolvedTheme } = useTheme();
  const shown = useTaskDebugStep(selection.task, selection.workflow);

  // Show the mapping as { path: logic }
  const visualData: Record<string, JsonLogicValue> = {
    [describeMappingPath(mapping.path)]: describeMappingValue(mapping),
  };

  const stepIndex = shown && shown.step.result !== 'skipped' ? shown.stepIndex : -1;
  const contextBefore = shown?.contextBefore;
  const mappingContextAt = shown?.mappingContextAt;

  // The editor's `data` is what this mapping was evaluated against: the
  // context after the mapping before it, which for the first is the context
  // before the step. Undefined when the step recorded no mapping ends, as a
  // failed map's may not.
  const debugData: Context | undefined = useMemo(() => {
    if (stepIndex < 0) return undefined;
    return mappingIndex === 0 ? contextBefore : mappingContextAt?.(stepIndex, mappingIndex - 1);
  }, [stepIndex, mappingIndex, contextBefore, mappingContextAt]);

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
