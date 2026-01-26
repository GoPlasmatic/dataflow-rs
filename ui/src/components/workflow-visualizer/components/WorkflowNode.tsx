import { Layers, GitBranch, ListTree } from 'lucide-react';
import type { Workflow, JsonLogicValue } from '../../../types';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import { useWorkflowDebugState, useWorkflowConditionDebugState } from '../hooks';
import { TreeNode } from './TreeNode';
import { TaskNode } from './TaskNode';
import { TREE_COLORS } from './colors';

interface WorkflowNodeProps {
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  expandedNodes: Set<string>;
  toggleNode: (id: string) => void;
  debugMode?: boolean;
}

function TasksNode({
  workflow,
  level,
  selection,
  onSelect,
  expandedNodes,
  toggleNode,
  debugMode = false,
}: {
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  expandedNodes: Set<string>;
  toggleNode: (id: string) => void;
  debugMode?: boolean;
}) {
  const nodeId = `tasks-${workflow.id}`;
  const isExpanded = expandedNodes.has(nodeId);

  return (
    <TreeNode
      label={`Tasks (${workflow.tasks.length})`}
      icon={<ListTree size={14} />}
      iconColor={TREE_COLORS.tasks}
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
          debugMode={debugMode}
        />
      ))}
    </TreeNode>
  );
}

export function WorkflowNode({
  workflow,
  level,
  selection,
  onSelect,
  expandedNodes,
  toggleNode,
  debugMode = false,
}: WorkflowNodeProps) {
  const workflowId = `workflow-${workflow.id}`;
  const isExpanded = expandedNodes.has(workflowId);
  const hasCondition = workflow.condition !== undefined && workflow.condition !== null && workflow.condition !== true;
  const hasTasks = workflow.tasks.length > 0;
  const hasChildren = hasCondition || hasTasks;

  // Get debug states
  const workflowDebugState = useWorkflowDebugState(workflow);
  const workflowConditionDebugState = useWorkflowConditionDebugState(workflow);

  return (
    <TreeNode
      label={workflow.name}
      icon={<Layers size={14} />}
      iconColor={TREE_COLORS.workflow}
      isExpanded={isExpanded}
      hasChildren={hasChildren}
      level={level}
      onToggle={() => toggleNode(workflowId)}
      onClick={() => toggleNode(workflowId)}
      debugState={debugMode ? workflowDebugState.state : null}
    >
      {/* Workflow Condition */}
      {hasCondition && (
        <TreeNode
          label="Condition"
          icon={<GitBranch size={14} />}
          iconColor={TREE_COLORS.condition}
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
          debugState={debugMode ? workflowConditionDebugState.state : null}
          conditionResult={debugMode ? workflowConditionDebugState.conditionResult : undefined}
          isCurrent={debugMode && workflowConditionDebugState.isCurrent}
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
          debugMode={debugMode}
        />
      )}
    </TreeNode>
  );
}
