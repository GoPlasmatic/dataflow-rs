import { useState, useCallback } from 'react';
import { Play, RefreshCw, FileJson, ChevronDown, ChevronRight } from 'lucide-react';
import { useDebugger } from '../context';

interface MessageInputPanelProps {
  /** Callback to trigger debug execution */
  onExecute: () => void;
  /** Whether execution is in progress */
  isExecuting?: boolean;
  /** Additional CSS class */
  className?: string;
}

type PayloadSection = 'payload';

/**
 * Panel for inputting test payload data
 *
 * Note: This is a simplified version that works with the new ExecutionTrace-based
 * debugging system. The input is a JSON payload that gets processed through workflows.
 */
export function MessageInputPanel({
  onExecute,
  isExecuting = false,
  className = '',
}: MessageInputPanelProps) {
  const { setInputPayload, state } = useDebugger();
  const { inputPayload } = state;

  const [expandedSections, setExpandedSections] = useState<Set<PayloadSection>>(
    new Set(['payload'])
  );
  const [editingSection, setEditingSection] = useState<PayloadSection | null>(null);
  const [editText, setEditText] = useState('');
  const [parseError, setParseError] = useState<string | null>(null);

  const payload = inputPayload || {};

  const toggleSection = (section: PayloadSection) => {
    setExpandedSections((prev) => {
      const next = new Set(prev);
      if (next.has(section)) {
        next.delete(section);
      } else {
        next.add(section);
      }
      return next;
    });
  };

  const startEditing = () => {
    setEditingSection('payload');
    setEditText(JSON.stringify(payload, null, 2));
    setParseError(null);
  };

  const cancelEditing = () => {
    setEditingSection(null);
    setEditText('');
    setParseError(null);
  };

  const saveEditing = useCallback(() => {
    if (!editingSection) return;

    try {
      const parsed = JSON.parse(editText);
      setInputPayload(parsed);
      setEditingSection(null);
      setEditText('');
      setParseError(null);
    } catch (err) {
      setParseError(err instanceof Error ? err.message : 'Invalid JSON');
    }
  }, [editingSection, editText, setInputPayload]);

  const handleLoadSample = () => {
    setInputPayload({
      id: '123',
      name: 'John Doe',
      email: 'john@example.com',
      amount: 100.00,
    });
  };

  const handleClear = () => {
    setInputPayload({});
  };

  const formatValue = (value: Record<string, unknown>): string => {
    if (Object.keys(value).length === 0) {
      return '(empty)';
    }
    return JSON.stringify(value, null, 2);
  };

  const isEmptyPayload = Object.keys(payload).length === 0;
  const isExpanded = expandedSections.has('payload');
  const isEditing = editingSection === 'payload';

  return (
    <div className={`df-debug-input-panel ${className}`}>
      <div className="df-debug-input-header">
        <span className="df-debug-input-title">Test Payload</span>
        <div className="df-debug-input-actions">
          <button
            className="df-debug-btn-sm"
            onClick={handleLoadSample}
            title="Load sample data"
          >
            <FileJson size={12} />
            Sample
          </button>
          <button
            className="df-debug-btn-sm"
            onClick={handleClear}
            title="Clear all data"
          >
            <RefreshCw size={12} />
            Clear
          </button>
        </div>
      </div>

      <div className="df-debug-input-sections">
        <div className="df-debug-input-section">
          <div
            className="df-debug-input-section-header"
            onClick={() => !isEditing && toggleSection('payload')}
          >
            <span className="df-debug-section-toggle">
              {isExpanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
            </span>
            <span className="df-debug-section-label">Payload</span>
            {isEmptyPayload && <span className="df-debug-section-empty">(empty)</span>}
            {!isEditing && (
              <button
                className="df-debug-section-edit"
                onClick={(e) => {
                  e.stopPropagation();
                  startEditing();
                }}
                title="Edit Payload"
              >
                Edit
              </button>
            )}
          </div>

          {isExpanded && (
            <div className="df-debug-input-section-content">
              {isEditing ? (
                <div className="df-debug-input-editor">
                  <textarea
                    className="df-debug-input-textarea"
                    value={editText}
                    onChange={(e) => setEditText(e.target.value)}
                    placeholder="Enter payload as JSON..."
                    spellCheck={false}
                  />
                  {parseError && (
                    <div className="df-debug-input-error">{parseError}</div>
                  )}
                  <div className="df-debug-input-editor-actions">
                    <button
                      className="df-debug-btn-sm df-debug-btn-cancel"
                      onClick={cancelEditing}
                    >
                      Cancel
                    </button>
                    <button
                      className="df-debug-btn-sm df-debug-btn-save"
                      onClick={saveEditing}
                    >
                      Save
                    </button>
                  </div>
                </div>
              ) : (
                <pre className="df-debug-input-preview">
                  {formatValue(payload)}
                </pre>
              )}
            </div>
          )}
        </div>
      </div>

      <div className="df-debug-input-footer">
        <button
          className="df-debug-run-btn"
          onClick={onExecute}
          disabled={isExecuting}
        >
          {isExecuting ? (
            <>
              <RefreshCw size={14} className="df-debug-spin" />
              Executing...
            </>
          ) : (
            <>
              <Play size={14} />
              Run Debug
            </>
          )}
        </button>
      </div>
    </div>
  );
}
