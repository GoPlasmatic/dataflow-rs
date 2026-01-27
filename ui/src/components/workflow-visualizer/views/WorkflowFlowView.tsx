import { useMemo } from 'react';
import { Play, CheckCircle, ArrowRight, Check, GitBranch, ExternalLink } from 'lucide-react';
import type { Workflow, Task, JsonLogicValue } from '../../../types';
import { FunctionTypeBadge } from '../cards/FunctionTypeBadge';
import type { TreeSelectionType } from '../WorkflowVisualizer';

interface WorkflowFlowViewProps {
  workflows: Workflow[];
  onTaskSelect?: (task: Task, workflow: Workflow) => void;
  onWorkflowSelect?: (workflow: Workflow) => void;
  onWorkflowConditionClick?: (workflow: Workflow) => void;
  onTaskConditionClick?: (task: Task, workflow: Workflow) => void;
  selection?: TreeSelectionType;
  highlightedTaskIds?: Set<string>;
}

// Condition indicator badge (clickable for conditional)
function ConditionIcon({
  condition,
  onClick,
  isSelected,
}: {
  condition: JsonLogicValue | undefined;
  onClick?: () => void;
  isSelected?: boolean;
}) {
  const isAlways = condition === undefined || condition === null || condition === true;
  const isClickable = !isAlways && onClick;

  return (
    <span
      className={`df-condition-icon ${isAlways ? 'df-condition-icon-always' : 'df-condition-icon-conditional'} ${isClickable ? 'df-condition-icon-clickable' : ''} ${isSelected ? 'df-condition-icon-selected' : ''}`}
      title={isAlways ? 'Always executes' : 'Click to view condition'}
      onClick={(e) => {
        if (isClickable) {
          e.stopPropagation();
          onClick();
        }
      }}
    >
      {isAlways ? <Check size={14} /> : <GitBranch size={14} />}
    </span>
  );
}

export function WorkflowFlowView({
  workflows,
  onTaskSelect,
  onWorkflowSelect,
  onWorkflowConditionClick,
  onTaskConditionClick,
  selection,
  highlightedTaskIds,
}: WorkflowFlowViewProps) {
  // Sort workflows by priority (lower number = higher priority = first)
  const sortedWorkflows = useMemo(() => {
    return [...workflows].sort((a, b) => (a.priority ?? 0) - (b.priority ?? 0));
  }, [workflows]);

  return (
    <div className="df-flow-view">
      <div className="df-flow-scroll">
        <div className="df-flow-container">
          {/* Start node */}
          <div className="df-flow-node-wrapper">
            <div className="df-flow-start-node">
              <Play size={18} />
              <span>Message</span>
            </div>
          </div>

          {/* Workflows */}
          {sortedWorkflows.map((workflow) => {
            const isWorkflowConditionSelected = selection?.type === 'workflow-condition' && selection.workflow.id === workflow.id;

            return (
              <div key={workflow.id} className="df-flow-node-wrapper">
                {/* Arrow */}
                <div className="df-flow-connector">
                  <ArrowRight size={18} />
                </div>

                {/* Workflow column */}
                <div className={`df-flow-column ${isWorkflowConditionSelected ? 'df-flow-column-selected' : ''}`}>
                  <div
                    className="df-flow-workflow-header"
                    onClick={() => onWorkflowSelect?.(workflow)}
                  >
                    <span className="df-flow-workflow-priority">P{workflow.priority ?? 0}</span>
                    <span className="df-flow-workflow-name">{workflow.name}</span>
                    <ConditionIcon
                      condition={workflow.condition}
                      onClick={() => onWorkflowConditionClick?.(workflow)}
                      isSelected={isWorkflowConditionSelected}
                    />
                  </div>

                  <div className="df-flow-tasks">
                    {workflow.tasks.map((task, index) => {
                      const isTaskSelected = selection?.type === 'task' && selection.task.id === task.id;
                      const isTaskConditionSelected = selection?.type === 'task-condition' && selection.task.id === task.id;

                      return (
                        <div
                          key={task.id}
                          className={`df-flow-task ${highlightedTaskIds?.has(task.id) ? 'df-flow-task-highlighted' : ''} ${isTaskSelected || isTaskConditionSelected ? 'df-flow-task-selected' : ''}`}
                        >
                          {/* Task header row with name and condition icon */}
                          <div className="df-flow-task-header">
                            <span className="df-flow-task-index">{index + 1}</span>
                            <span className="df-flow-task-name">{task.name}</span>
                            <ConditionIcon
                              condition={task.condition}
                              onClick={() => onTaskConditionClick?.(task, workflow)}
                              isSelected={isTaskConditionSelected}
                            />
                          </div>

                          {/* Task function type with details link */}
                          <div className="df-flow-task-function">
                            <FunctionTypeBadge functionName={task.function.name} />
                            <button
                              className="df-flow-task-details-link"
                              onClick={(e) => {
                                e.stopPropagation();
                                onTaskSelect?.(task, workflow);
                              }}
                              title="View task details"
                            >
                              <span>Details</span>
                              <ExternalLink size={12} />
                            </button>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                </div>
              </div>
            );
          })}

          {/* End node */}
          <div className="df-flow-node-wrapper">
            <div className="df-flow-connector">
              <ArrowRight size={18} />
            </div>
            <div className="df-flow-end-node">
              <CheckCircle size={18} />
              <span>Done</span>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
