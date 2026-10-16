import { useMemo } from 'react';
import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
// The replay lives in the engine: `mapping_contexts` rebuilds the context
// after each of a map step's mappings from its `changes` and `mapping_ends`.
import { mapping_contexts } from '@goplasmatic/dataflow-wasm';
import type { Context, JsonLogicValue } from '../../../../types';
import { contextAtStep } from '../../../../types';
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

  // One replay per step, not per mapping node: the engine gives the context
  // after every mapping of the step at once (an empty array for a step that
  // is not a map's, or recorded no changes).
  const trace = shown?.trace ?? null;
  const stepIndex = shown && shown.step.result !== 'skipped' ? shown.stepIndex : -1;
  const mappingContexts = useMemo(
    () => (trace && stepIndex >= 0 ? mapping_contexts(trace, stepIndex) : []),
    [trace, stepIndex]
  );

  // The editor's `data` is what this mapping was evaluated against: the
  // context after the mapping before it, which for the first is the context
  // before the step. Undefined when the step recorded no mapping ends, as a
  // failed map's may not.
  let debugData: Context | undefined;
  if (trace && shown && stepIndex >= 0) {
    debugData = mappingIndex === 0
      ? contextAtStep(trace, shown.contexts, stepIndex - 1)
      : mappingContexts[mappingIndex - 1];
  }

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
