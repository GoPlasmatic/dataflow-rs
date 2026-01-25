import { useState, useMemo } from 'react';
import {
  ChevronDown,
  ChevronRight,
  GitBranch,
  Layers,
  Box,
  ArrowRightLeft,
  CheckCircle,
  ListTree,
} from 'lucide-react';
import type { Workflow, Task, JsonLogicValue } from '../../../types';
import type { TreeSelectionType } from '../WorkflowVisualizer';

interface TreeViewProps {
  workflows: Workflow[];
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
}

interface TreeNodeProps {
  label: string;
  icon?: React.ReactNode;
  iconColor?: string;
  isExpanded?: boolean;
  isSelected?: boolean;
  hasChildren?: boolean;
  level: number;
  onToggle?: () => void;
  onClick?: () => void;
  children?: React.ReactNode;
}

function TreeNode({
  label,
  icon,
  iconColor,
  isExpanded = false,
  isSelected = false,
  hasChildren = false,
  level,
  onToggle,
  onClick,
  children,
}: TreeNodeProps) {
  const handleClick = (e: React.MouseEvent) => {
    e.stopPropagation();
    onClick?.();
  };

  const handleToggle = (e: React.MouseEvent) => {
    e.stopPropagation();
    onToggle?.();
  };

  return (
    <div className="df-tree-node">
      <div
        className={`df-tree-node-content ${isSelected ? 'df-tree-node-selected' : ''}`}
        style={{ paddingLeft: `${level * 16 + 8}px` }}
        onClick={handleClick}
      >
        <span
          className="df-tree-toggle"
          onClick={hasChildren ? handleToggle : undefined}
          style={{ visibility: hasChildren ? 'visible' : 'hidden' }}
        >
          {isExpanded ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
        </span>
        {icon && <span className="df-tree-icon" style={iconColor ? { color: iconColor } : undefined}>{icon}</span>}
        <span className="df-tree-label">{label}</span>
      </div>
      {isExpanded && hasChildren && (
        <div className="df-tree-children">{children}</div>
      )}
    </div>
  );
}

interface MappingItem {
  path: string;
  logic: JsonLogicValue;
}

interface ValidationRule {
  logic: JsonLogicValue;
  message: string;
}

// Color constants for tree icons - VSCode inspired
const COLORS = {
  workflow: '#0078d4',      // VSCode blue
  condition: '#dcdcaa',     // VSCode yellow/warning
  task: '#c586c0',          // VSCode purple/pink
  mapping: '#4ec9b0',       // VSCode teal/green
  validation: '#ce9178',    // VSCode orange
  tasks: '#9d9d9d',         // VSCode gray
};

function TaskNode({
  task,
  workflow,
  level,
  selection,
  onSelect,
  expandedNodes,
  toggleNode,
}: {
  task: Task;
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  expandedNodes: Set<string>;
  toggleNode: (id: string) => void;
}) {
  const taskId = `task-${workflow.id}-${task.id}`;
  const isExpanded = expandedNodes.has(taskId);
  const functionName = task.function.name;
  const input = task.function.input as Record<string, unknown> | undefined;

  const hasCondition = task.condition !== undefined && task.condition !== null && task.condition !== true;
  const mappings = functionName === 'map' ? (input?.mappings as MappingItem[]) || [] : [];
  const rules = functionName === 'validation' ? (input?.rules as ValidationRule[]) || [] : [];
  const hasChildren = hasCondition || mappings.length > 0 || rules.length > 0;

  const isTaskSelected = selection.type === 'task' &&
    selection.task.id === task.id &&
    selection.workflow.id === workflow.id;

  return (
    <TreeNode
      label={task.name}
      icon={<Box size={14} />}
      iconColor={COLORS.task}
      isExpanded={isExpanded}
      isSelected={isTaskSelected}
      hasChildren={hasChildren}
      level={level}
      onToggle={() => toggleNode(taskId)}
      onClick={() => onSelect({ type: 'task', task, workflow })}
    >
      {/* Task Condition */}
      {hasCondition && (
        <TreeNode
          label="Condition"
          icon={<GitBranch size={14} />}
          iconColor={COLORS.condition}
          level={level + 1}
          isSelected={
            selection.type === 'task-condition' &&
            selection.task.id === task.id &&
            selection.workflow.id === workflow.id
          }
          onClick={() =>
            onSelect({
              type: 'task-condition',
              task,
              workflow,
              condition: task.condition as JsonLogicValue,
            })
          }
        />
      )}

      {/* Mappings for map function - directly show each mapping */}
      {mappings.map((mapping, index) => (
        <TreeNode
          key={`mapping-${index}`}
          label={mapping.path}
          icon={<ArrowRightLeft size={14} />}
          iconColor={COLORS.mapping}
          level={level + 1}
          isSelected={
            selection.type === 'mapping' &&
            selection.task.id === task.id &&
            selection.workflow.id === workflow.id &&
            selection.mappingIndex === index
          }
          onClick={() =>
            onSelect({
              type: 'mapping',
              task,
              workflow,
              mapping,
              mappingIndex: index,
            })
          }
        />
      ))}

      {/* Validations for validation function - directly show each rule */}
      {rules.map((rule, index) => (
        <TreeNode
          key={`rule-${index}`}
          label={rule.message}
          icon={<CheckCircle size={14} />}
          iconColor={COLORS.validation}
          level={level + 1}
          isSelected={
            selection.type === 'validation-rule' &&
            selection.task.id === task.id &&
            selection.workflow.id === workflow.id &&
            selection.ruleIndex === index
          }
          onClick={() =>
            onSelect({
              type: 'validation-rule',
              task,
              workflow,
              rule,
              ruleIndex: index,
            })
          }
        />
      ))}
    </TreeNode>
  );
}

function WorkflowNode({
  workflow,
  level,
  selection,
  onSelect,
  expandedNodes,
  toggleNode,
}: {
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  expandedNodes: Set<string>;
  toggleNode: (id: string) => void;
}) {
  const workflowId = `workflow-${workflow.id}`;
  const isExpanded = expandedNodes.has(workflowId);
  const hasCondition = workflow.condition !== undefined && workflow.condition !== null && workflow.condition !== true;
  const hasTasks = workflow.tasks.length > 0;
  const hasChildren = hasCondition || hasTasks;

  return (
    <TreeNode
      label={workflow.name}
      icon={<Layers size={14} />}
      iconColor={COLORS.workflow}
      isExpanded={isExpanded}
      hasChildren={hasChildren}
      level={level}
      onToggle={() => toggleNode(workflowId)}
      onClick={() => toggleNode(workflowId)}
    >
      {/* Workflow Condition */}
      {hasCondition && (
        <TreeNode
          label="Condition"
          icon={<GitBranch size={14} />}
          iconColor={COLORS.condition}
          level={level + 1}
          isSelected={
            selection.type === 'workflow-condition' &&
            selection.workflow.id === workflow.id
          }
          onClick={() =>
            onSelect({
              type: 'workflow-condition',
              workflow,
              condition: workflow.condition as JsonLogicValue,
            })
          }
        />
      )}

      {/* Tasks */}
      {hasTasks && (
        <TasksNode
          workflow={workflow}
          level={level + 1}
          selection={selection}
          onSelect={onSelect}
          expandedNodes={expandedNodes}
          toggleNode={toggleNode}
        />
      )}
    </TreeNode>
  );
}

function TasksNode({
  workflow,
  level,
  selection,
  onSelect,
  expandedNodes,
  toggleNode,
}: {
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  expandedNodes: Set<string>;
  toggleNode: (id: string) => void;
}) {
  const nodeId = `tasks-${workflow.id}`;
  const isExpanded = expandedNodes.has(nodeId);

  return (
    <TreeNode
      label={`Tasks (${workflow.tasks.length})`}
      icon={<ListTree size={14} />}
      iconColor={COLORS.tasks}
      isExpanded={isExpanded}
      hasChildren={workflow.tasks.length > 0}
      level={level}
      onToggle={() => toggleNode(nodeId)}
      onClick={() => toggleNode(nodeId)}
    >
      {workflow.tasks.map((task) => (
        <TaskNode
          key={task.id}
          task={task}
          workflow={workflow}
          level={level + 1}
          selection={selection}
          onSelect={onSelect}
          expandedNodes={expandedNodes}
          toggleNode={toggleNode}
        />
      ))}
    </TreeNode>
  );
}

export function TreeView({ workflows, selection, onSelect }: TreeViewProps) {
  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(() => {
    // Initially expand the root "Workflows" node and first workflow
    const initial = new Set(['workflows-root']);
    if (workflows.length > 0) {
      initial.add(`workflow-${workflows[0].id}`);
    }
    return initial;
  });

  const sortedWorkflows = useMemo(() => {
    return [...workflows].sort((a, b) => (a.priority ?? 0) - (b.priority ?? 0));
  }, [workflows]);

  const toggleNode = (id: string) => {
    setExpandedNodes((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const isRootExpanded = expandedNodes.has('workflows-root');

  return (
    <div className="df-tree-view">
      <TreeNode
        label="Workflows"
        icon={<Layers size={14} />}
        iconColor={COLORS.workflow}
        isExpanded={isRootExpanded}
        hasChildren={sortedWorkflows.length > 0}
        level={0}
        onToggle={() => toggleNode('workflows-root')}
        onClick={() => toggleNode('workflows-root')}
      >
        {sortedWorkflows.map((workflow) => (
          <WorkflowNode
            key={workflow.id}
            workflow={workflow}
            level={1}
            selection={selection}
            onSelect={onSelect}
            expandedNodes={expandedNodes}
            toggleNode={toggleNode}
          />
        ))}
      </TreeNode>
    </div>
  );
}
