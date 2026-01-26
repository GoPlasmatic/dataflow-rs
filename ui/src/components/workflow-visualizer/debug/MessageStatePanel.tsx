import { useState } from 'react';
import { useDebugger } from '../context';
import type { Message, Change } from '../../../types';

type ViewTab = 'message' | 'changes';

interface MessageStatePanelProps {
  /** Additional CSS class */
  className?: string;
}

/**
 * Panel showing message state at current execution step
 */
export function MessageStatePanel({ className = '' }: MessageStatePanelProps) {
  const { currentStep, currentMessage, currentChanges } = useDebugger();
  const [activeTab, setActiveTab] = useState<ViewTab>('message');

  if (!currentStep || !currentMessage) {
    return (
      <div className={`df-debug-state-panel df-debug-state-empty ${className}`}>
        <p>No step selected. Run debug to see message state changes.</p>
      </div>
    );
  }

  const { workflow_id, task_id, result } = currentStep;

  const tabs: { key: ViewTab; label: string }[] = [
    { key: 'message', label: 'Message' },
    { key: 'changes', label: `Changes (${currentChanges.length})` },
  ];

  return (
    <div className={`df-debug-state-panel ${className}`}>
      <div className="df-debug-state-header">
        <div className="df-debug-state-tabs">
          {tabs.map(({ key, label }) => (
            <button
              key={key}
              className={`df-debug-state-tab ${activeTab === key ? 'df-debug-state-tab-active' : ''}`}
              onClick={() => setActiveTab(key)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      <div className="df-debug-state-content">
        {activeTab === 'message' && (
          <MessageView message={currentMessage} />
        )}
        {activeTab === 'changes' && (
          <ChangesView changes={currentChanges} />
        )}
      </div>

      {/* Step info footer */}
      <div className="df-debug-state-footer">
        <div className="df-debug-state-step-type">
          <span className={`df-debug-state-badge df-debug-state-badge-${result}`}>
            {result}
          </span>
          <span className="df-debug-state-type-label">
            {workflow_id}{task_id ? ` / ${task_id}` : ''}
          </span>
        </div>
        {currentMessage.errors.length > 0 && (
          <div className="df-debug-state-error">
            Errors: {currentMessage.errors.length}
          </div>
        )}
      </div>
    </div>
  );
}

interface MessageViewProps {
  message: Message;
}

function MessageView({ message }: MessageViewProps) {
  const [expandedSections, setExpandedSections] = useState<Set<string>>(
    new Set(['context'])
  );

  const toggleSection = (section: string) => {
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

  const sections = [
    { key: 'context', value: message.context },
    { key: 'payload', value: message.payload },
    { key: 'errors', value: message.errors },
    { key: 'audit_trail', value: message.audit_trail },
  ];

  return (
    <div className="df-debug-message-view">
      {sections.map(({ key, value }) => {
        const isExpanded = expandedSections.has(key);
        const isEmpty = Array.isArray(value)
          ? value.length === 0
          : typeof value === 'object' && value !== null
            ? Object.keys(value).length === 0
            : !value;

        return (
          <div key={key} className="df-debug-message-section">
            <div
              className="df-debug-message-section-header"
              onClick={() => toggleSection(key)}
            >
              <span className="df-debug-section-arrow">
                {isExpanded ? '▼' : '▶'}
              </span>
              <span className="df-debug-section-key">{key}</span>
              {isEmpty && <span className="df-debug-section-empty">(empty)</span>}
            </div>
            {isExpanded && (
              <pre className="df-debug-message-section-content">
                {JSON.stringify(value, null, 2)}
              </pre>
            )}
          </div>
        );
      })}
    </div>
  );
}

interface ChangesViewProps {
  changes: Change[];
}

function ChangesView({ changes }: ChangesViewProps) {
  if (changes.length === 0) {
    return (
      <div className="df-debug-changes-empty">
        No changes recorded in this step
      </div>
    );
  }

  return (
    <div className="df-debug-changes-view">
      {changes.map((change, index) => {
        const isModified = change.old_value !== undefined && change.new_value !== undefined;
        const isAdded = change.old_value === undefined && change.new_value !== undefined;
        const isRemoved = change.old_value !== undefined && change.new_value === undefined;
        const changeType = isAdded ? 'added' : isRemoved ? 'removed' : 'modified';

        return (
          <div key={index} className={`df-debug-change-item df-debug-change-${changeType}`}>
            <div className="df-debug-change-header">
              <span className={`df-debug-change-op df-debug-change-op-${changeType}`}>
                {changeType}
              </span>
              <span className="df-debug-change-path">{change.path}</span>
            </div>
            {isModified && (
              <div className="df-debug-change-values">
                <span className="df-debug-change-old">{formatValue(change.old_value)}</span>
                <span className="df-debug-change-arrow">→</span>
                <span className="df-debug-change-new">{formatValue(change.new_value)}</span>
              </div>
            )}
            {isAdded && (
              <div className="df-debug-change-values">
                <span className="df-debug-change-new">{formatValue(change.new_value)}</span>
              </div>
            )}
            {isRemoved && (
              <div className="df-debug-change-values">
                <span className="df-debug-change-old">{formatValue(change.old_value)}</span>
              </div>
            )}
          </div>
        );
      })}
    </div>
  );
}

function formatValue(value: unknown): string {
  if (value === undefined) return 'undefined';
  if (value === null) return 'null';
  if (typeof value === 'string') return `"${value}"`;
  if (typeof value === 'object') return JSON.stringify(value);
  return String(value);
}
