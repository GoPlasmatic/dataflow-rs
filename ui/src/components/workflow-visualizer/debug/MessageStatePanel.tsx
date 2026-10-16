import { useState } from 'react';
import { useDebugger } from '../context';
import type { Change, Context, ErrorRecord, Message, TraceStep } from '../../../types';
import { isEngineStep } from '../../../types';

type ViewTab = 'context' | 'changes' | 'errors' | 'message';

interface MessageStatePanelProps {
  /** Additional CSS class */
  className?: string;
}

/**
 * Where a step happened, in one line: its workflow, its task, and the sweep
 * or fan-out element it ran for. The engine's own step is the loop counter
 * it wrote, and no more: a sweep may follow it, or the loop may end on it,
 * and the step does not say which.
 */
function describeStep(step: TraceStep): string {
  if (isEngineStep(step)) {
    return step.loop_counter !== undefined
      ? `${step.workflow_id} · loop counter ${step.loop_counter}`
      : `${step.workflow_id} · engine`;
  }
  let text = step.workflow_id;
  if (step.task_id) text += ` / ${step.task_id}`;
  if (step.loop_counter !== undefined) text += ` · sweep ${step.loop_counter}`;
  if (step.element_index !== undefined) text += ` · element ${step.element_index}`;
  return text;
}

/**
 * Panel showing the context at the current execution step, the step's
 * changes and errors, and, at the end of the trace, the final message.
 *
 * A dataflow-rs 4 trace keeps no message per step: the context shown is the
 * engine's replay of the steps' changes, and the final `errors` and
 * `audit_trail` exist only on the message the run returned.
 */
export function MessageStatePanel({ className = '' }: MessageStatePanelProps) {
  const { state, currentStep, currentContext, currentChanges, currentErrors, isAtEnd } = useDebugger();
  const [activeTab, setActiveTab] = useState<ViewTab>('context');
  const { trace, message } = state;

  if (!trace || !currentContext) {
    return (
      <div className={`df-debug-state-panel df-debug-state-empty ${className}`}>
        <p>No trace yet. Run debug to see the context change step by step.</p>
      </div>
    );
  }

  // The final message is only meaningful at the end: before that, its
  // errors and audit trail are from steps not yet reached.
  const showMessage = isAtEnd && message !== null;
  const tab: ViewTab = activeTab === 'message' && !showMessage ? 'context' : activeTab;

  const tabs: { key: ViewTab; label: string }[] = [
    { key: 'context', label: 'Context' },
    { key: 'changes', label: `Changes (${currentChanges.length})` },
    { key: 'errors', label: `Errors (${currentErrors.length})` },
  ];
  if (showMessage) {
    tabs.push({ key: 'message', label: 'Final message' });
  }

  // `failed` has no badge of its own: it is an error.
  const badge = currentStep
    ? currentStep.result === 'failed' ? 'error' : currentStep.result
    : 'pending';

  return (
    <div className={`df-debug-state-panel ${className}`}>
      <div className="df-debug-state-header">
        <div className="df-debug-state-tabs">
          {tabs.map(({ key, label }) => (
            <button
              key={key}
              className={`df-debug-state-tab ${tab === key ? 'df-debug-state-tab-active' : ''}`}
              onClick={() => setActiveTab(key)}
            >
              {label}
            </button>
          ))}
        </div>
      </div>

      {trace.truncated && (
        <div className="df-debug-state-note">
          The trace crossed its byte budget: steps past it carry no changes, so the
          context shown is the one before the run.
        </div>
      )}

      <div className="df-debug-state-content">
        {tab === 'context' && <ContextView context={currentContext} />}
        {tab === 'changes' && <ChangesView changes={currentChanges} />}
        {tab === 'errors' && <ErrorsView errors={currentErrors} />}
        {tab === 'message' && message && <FinalMessageView message={message} />}
      </div>

      {/* Step info footer */}
      <div className="df-debug-state-footer">
        <div className="df-debug-state-step-type">
          <span className={`df-debug-state-badge df-debug-state-badge-${badge}`}>
            {currentStep ? currentStep.result : 'ready'}
          </span>
          {currentStep?.status !== null && currentStep?.status !== undefined && (
            <span className="df-debug-state-status" title="The call's status code">
              {currentStep.status}
            </span>
          )}
          <span className="df-debug-state-type-label">
            {currentStep ? describeStep(currentStep) : 'before the first step'}
          </span>
        </div>
        {currentErrors.length > 0 && (
          <div className="df-debug-state-error">
            Errors: {currentErrors.length}
          </div>
        )}
      </div>
    </div>
  );
}

interface JsonSectionsProps {
  sections: { key: string; value: unknown }[];
  /** The sections open at first */
  initiallyExpanded: string[];
}

/**
 * Collapsible JSON sections, one per key.
 */
function JsonSections({ sections, initiallyExpanded }: JsonSectionsProps) {
  const [expandedSections, setExpandedSections] = useState<Set<string>>(
    () => new Set(initiallyExpanded)
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

function ContextView({ context }: { context: Context }) {
  return (
    <JsonSections
      sections={[
        { key: 'data', value: context.data },
        { key: 'metadata', value: context.metadata },
        { key: 'temp_data', value: context.temp_data },
      ]}
      initiallyExpanded={['data', 'metadata', 'temp_data']}
    />
  );
}

function FinalMessageView({ message }: { message: Message }) {
  return (
    <JsonSections
      sections={[
        { key: 'errors', value: message.errors },
        { key: 'audit_trail', value: message.audit_trail },
      ]}
      initiallyExpanded={['errors']}
    />
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
        // A removal says so; otherwise a path that was absent before the
        // write records `old_value` as `null`, so a write over an explicit
        // `null` reads as an addition too: the wire cannot tell them apart.
        const op = change.removed
          ? 'delete'
          : change.old_value === null
            ? 'add'
            : 'modify';

        return (
          <div key={index} className="df-debug-change-item">
            <div className="df-debug-change-header">
              <span className={`df-debug-change-op df-debug-change-op-${op}`}>
                {op}
              </span>
              <span className="df-debug-change-path">{change.path}</span>
              {change.replaced && (
                <span className="df-debug-change-source" title="The write replaced the root rather than merging into it">
                  replaced
                </span>
              )}
            </div>
            {op === 'modify' && (
              <div className="df-debug-change-values">
                <span className="df-debug-change-old">{formatValue(change.old_value)}</span>
                <span className="df-debug-change-arrow">→</span>
                <span className="df-debug-change-new">{formatValue(change.new_value)}</span>
              </div>
            )}
            {op === 'add' && (
              <div className="df-debug-change-values">
                <span className="df-debug-change-new">{formatValue(change.new_value)}</span>
              </div>
            )}
            {op === 'delete' && (
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

interface ErrorsViewProps {
  errors: ErrorRecord[];
}

/**
 * The records a step added to the message: a validation's per-rule records,
 * a function's own, then the engine's.
 */
function ErrorsView({ errors }: ErrorsViewProps) {
  if (errors.length === 0) {
    return (
      <div className="df-debug-changes-empty">
        No errors recorded in this step
      </div>
    );
  }

  return (
    <div className="df-debug-errors-view">
      {errors.map((error, index) => (
        <div key={index} className="df-debug-error-item">
          <div className="df-debug-error-header">
            <span className="df-debug-error-code">{error.code}</span>
            {error.path && <span className="df-debug-error-path">{error.path}</span>}
            {error.element_index !== undefined && (
              <span className="df-debug-error-where">element {error.element_index}</span>
            )}
          </div>
          <div className="df-debug-error-message">{error.message}</div>
        </div>
      ))}
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
