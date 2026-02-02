import { ChevronRight } from 'lucide-react';
import type { Task, Workflow, MapFunctionInput, ValidationFunctionInput } from '../../../types';
import { FunctionTypeBadge } from './FunctionTypeBadge';
import { ConditionBadge } from './ConditionBadge';

interface TaskRowProps {
  task: Task;
  workflow: Workflow;
  index: number;
  isHighlighted?: boolean;
  onSelect?: (task: Task, workflow: Workflow) => void;
}

function getTaskSubtitle(task: Task): string | null {
  const { function: fn } = task;

  if (fn.name === 'map' && fn.input) {
    const input = fn.input as unknown as MapFunctionInput;
    if (input.mappings) {
      const count = input.mappings.length;
      return `${count} mapping${count !== 1 ? 's' : ''}`;
    }
  }

  if ((fn.name === 'validation' || fn.name === 'validate') && fn.input) {
    const input = fn.input as unknown as ValidationFunctionInput;
    if (input.rules) {
      const count = input.rules.length;
      return `${count} rule${count !== 1 ? 's' : ''}`;
    }
  }

  return null;
}

export function TaskRow({
  task,
  workflow,
  index,
  isHighlighted = false,
  onSelect,
}: TaskRowProps) {
  const subtitle = getTaskSubtitle(task);

  return (
    <div
      className={`df-task-row ${isHighlighted ? 'df-task-row-highlighted' : ''}`}
      onClick={() => onSelect?.(task, workflow)}
      role="button"
      tabIndex={0}
      onKeyDown={(e) => {
        if (e.key === 'Enter' || e.key === ' ') {
          e.preventDefault();
          onSelect?.(task, workflow);
        }
      }}
    >
      <span className="df-task-index">{index + 1}.</span>

      <div className="df-task-info">
        <div className="df-task-name-row">
          <span className="df-task-name">{task.name}</span>
          <FunctionTypeBadge functionName={task.function.name} />
        </div>
        {subtitle && <span className="df-task-subtitle">{subtitle}</span>}
      </div>

      <ConditionBadge condition={task.condition} />

      <ChevronRight className="df-task-chevron" size={16} />
    </div>
  );
}
