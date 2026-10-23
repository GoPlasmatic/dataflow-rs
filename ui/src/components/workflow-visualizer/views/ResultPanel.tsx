import { useMemo } from 'react';
import type { Change, Context, Message } from '../../../types';
import { JsonEditor } from '../../common';

interface ResultPanelProps {
  /**
   * What to show: the context at the debugger's step (`trace.initial` before
   * the first), or, outside the debugger, a host's result message.
   */
  displayValue: Message | Context;
  /** Current step index (-1 means ready state) */
  currentStepIndex: number;
  /** Changes at the current step */
  currentChanges: Change[];
  /** Theme for the editor */
  theme: 'light' | 'dark';
  /** Whether we're displaying debug trace data */
  hasDebugTrace: boolean;
}

const NOOP_CHANGE = () => {};

export function ResultPanel({
  displayValue,
  currentStepIndex,
  currentChanges,
  theme,
  hasDebugTrace,
}: ResultPanelProps) {
  const formattedJson = useMemo(() => JSON.stringify(displayValue, null, 2), [displayValue]);

  const highlightedPaths = useMemo(() => {
    if (!currentChanges || currentChanges.length === 0) return undefined;
    // `Change.path` is rooted at the context (`data.user.name`), and so is
    // the JSON rendered here whenever there are changes to highlight: the
    // debugger shows the context itself, not a message wrapping it.
    return currentChanges.map((change: Change) => change.path);
  }, [currentChanges]);

  return (
    <>
      <div className="df-visualizer-result-header">
        <span className="df-visualizer-result-title">
          {hasDebugTrace
            ? (currentStepIndex >= 0
                ? `Step ${currentStepIndex + 1}`
                : 'Initial context')
            : 'Result'}
        </span>
        {hasDebugTrace && currentChanges && currentChanges.length > 0 && (
          <span className="df-visualizer-result-changes">
            {currentChanges.length} change{currentChanges.length !== 1 ? 's' : ''}
          </span>
        )}
      </div>

      <div className="df-visualizer-result-content">
        <JsonEditor
          value={formattedJson}
          onChange={NOOP_CHANGE}
          readOnly={true}
          theme={theme}
          highlightedPaths={highlightedPaths}
        />
      </div>
    </>
  );
}
