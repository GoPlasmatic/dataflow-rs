import { Layers, GitBranch } from 'lucide-react';
import type { Workflow, JsonLogicValue } from '../../../types';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import { useWorkflowDebugState, useWorkflowConditionDebugState } from '../hooks';
import { TreeNode } from './TreeNode';
import { TaskNode } from './TaskNode';
import { TREE_COLORS } from './colors';
import { NODE_IDS } from '../constants';

interface WorkflowNodeProps {
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  expandedNodes: Set<string>;
  toggleNode: (id: string) => void;
  debugMode?: boolean;
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
  const workflowId = NODE_IDS.workflow(workflow.id);
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
      isSelected={selection.type === 'workflow' && selection.workflow.id === workflow.id}
      hasChildren={hasChildren}
      level={level}
      onToggle={() => toggleNode(workflowId)}
      onClick={() => {
        onSelect({ type: 'workflow', workflow });
        if (!isExpanded) toggleNode(workflowId);
      }}
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

      {/* Tasks - directly under workflow */}
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
