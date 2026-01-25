import { useMemo } from 'react';
import type { Workflow, Task } from '../../../types';
import { WorkflowCard } from '../cards';
import { useExpandedState } from '../hooks';

interface RulesListViewProps {
  workflows: Workflow[];
  highlightedTaskIds?: Set<string>;
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  defaultExpandedIds?: string[];
}

export function RulesListView({
  workflows,
  highlightedTaskIds,
  onTaskSelect,
  defaultExpandedIds,
}: RulesListViewProps) {
  // Sort workflows by priority
  const sortedWorkflows = useMemo(() => {
    return [...workflows].sort((a, b) => (a.priority ?? 0) - (b.priority ?? 0));
  }, [workflows]);

  const { isExpanded, toggle, expandAll, collapseAll } = useExpandedState(
    defaultExpandedIds ?? (sortedWorkflows.length > 0 ? [sortedWorkflows[0].id] : [])
  );

  const allIds = useMemo(() => sortedWorkflows.map((w) => w.id), [sortedWorkflows]);

  return (
    <div className="df-rules-list-view">
      <div className="df-rules-list-actions">
        <button
          className="df-action-button"
          onClick={() => expandAll(allIds)}
        >
          Expand All
        </button>
        <button
          className="df-action-button"
          onClick={collapseAll}
        >
          Collapse All
        </button>
      </div>

      <div className="df-workflow-list">
        {sortedWorkflows.length === 0 ? (
          <div className="df-empty-state">
            <p>No workflows found</p>
          </div>
        ) : (
          sortedWorkflows.map((workflow) => (
            <WorkflowCard
              key={workflow.id}
              workflow={workflow}
              isExpanded={isExpanded(workflow.id)}
              onToggle={() => toggle(workflow.id)}
              onTaskSelect={onTaskSelect}
              highlightedTaskIds={highlightedTaskIds}
            />
          ))
        )}
      </div>
    </div>
  );
}
