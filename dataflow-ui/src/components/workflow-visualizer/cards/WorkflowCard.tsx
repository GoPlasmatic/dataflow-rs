import { ChevronDown, ChevronRight } from 'lucide-react';
import type { Workflow } from '../../../types';
import { TaskRow } from './TaskRow';
import { ConditionBadge } from './ConditionBadge';

interface WorkflowCardProps {
  workflow: Workflow;
  isExpanded: boolean;
  onToggle: () => void;
  onTaskSelect?: (task: Workflow['tasks'][0], workflow: Workflow) => void;
  highlightedTaskIds?: Set<string>;
}

export function WorkflowCard({
  workflow,
  isExpanded,
  onToggle,
  onTaskSelect,
  highlightedTaskIds,
}: WorkflowCardProps) {
  const priorityDisplay = workflow.priority ?? 0;

  return (
    <div className={`df-workflow-card ${isExpanded ? 'df-workflow-card-expanded' : ''}`}>
      <div
        className="df-workflow-header"
        onClick={onToggle}
        role="button"
        tabIndex={0}
        onKeyDown={(e) => {
          if (e.key === 'Enter' || e.key === ' ') {
            e.preventDefault();
            onToggle();
          }
        }}
      >
        <span className="df-workflow-toggle">
          {isExpanded ? <ChevronDown size={18} /> : <ChevronRight size={18} />}
        </span>

        <div className="df-workflow-title">
          <span className="df-workflow-name">{workflow.name}</span>
          {workflow.description && (
            <span className="df-workflow-description">{workflow.description}</span>
          )}
        </div>

        <div className="df-workflow-meta">
          <span className="df-workflow-priority">Priority: {priorityDisplay}</span>
          <span className="df-workflow-task-count">
            {workflow.tasks.length} task{workflow.tasks.length !== 1 ? 's' : ''}
          </span>
        </div>
      </div>

      {isExpanded && (
        <div className="df-workflow-body">
          <div className="df-workflow-condition-section">
            <span className="df-section-label">Condition:</span>
            <div className="df-workflow-condition">
              {workflow.condition ? (
                <pre className="df-condition-code">
                  {JSON.stringify(workflow.condition, null, 2)}
                </pre>
              ) : (
                <span className="df-condition-always-text">Always executes (no condition)</span>
              )}
            </div>
          </div>

          <div className="df-workflow-tasks-section">
            <span className="df-section-label">Tasks:</span>
            <div className="df-task-list">
              {workflow.tasks.map((task, index) => (
                <TaskRow
                  key={task.id}
                  task={task}
                  workflow={workflow}
                  index={index}
                  isHighlighted={highlightedTaskIds?.has(task.id)}
                  onSelect={onTaskSelect}
                />
              ))}
            </div>
          </div>

          {workflow.continue_on_error && (
            <div className="df-workflow-footer">
              <span className="df-continue-on-error">
                Continue on error enabled
              </span>
            </div>
          )}
        </div>
      )}

      {!isExpanded && (
        <div className="df-workflow-collapsed-info">
          <ConditionBadge condition={workflow.condition} />
        </div>
      )}
    </div>
  );
}
