import { useMemo } from 'react';
import type { Message, Change } from '../../../types';
import { JsonEditor } from '../../common';

interface ResultPanelProps {
  /** The message to display */
  displayMessage: Message;
  /** Current step index (-1 means ready state) */
  currentStepIndex: number;
  /** Changes at the current step */
  currentChanges: Change[];
  /** Theme for the editor */
  theme: 'light' | 'dark';
  /** Whether we're displaying debug trace data */
  hasDebugTrace: boolean;
}

export function ResultPanel({
  displayMessage,
  currentStepIndex,
  currentChanges,
  theme,
  hasDebugTrace,
}: ResultPanelProps) {
  const highlightedPaths = useMemo(() => {
    if (!currentChanges || currentChanges.length === 0) return undefined;
    return currentChanges.map((change: Change) => change.path);
  }, [currentChanges]);

  return (
    <>
      <div className="df-visualizer-result-header">
        <span className="df-visualizer-result-title">
          {hasDebugTrace
            ? (currentStepIndex >= 0
                ? `Step ${currentStepIndex + 1}`
                : 'Ready')
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
          value={JSON.stringify(displayMessage, null, 2)}
          onChange={() => {}}
          readOnly={true}
          theme={theme}
          highlightedPaths={highlightedPaths}
        />
      </div>
    </>
  );
}
