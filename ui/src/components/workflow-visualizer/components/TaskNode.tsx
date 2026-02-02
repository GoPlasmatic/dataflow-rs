import { Box, GitBranch, ArrowRightLeft, CheckCircle } from 'lucide-react';
import type { Workflow, Task, JsonLogicValue, MappingItem, ValidationRule } from '../../../types';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import { useTaskDebugState, useTaskConditionDebugState, useMappingDebugState, useValidationRuleDebugState } from '../hooks';
import { TreeNode } from './TreeNode';
import { TREE_COLORS } from './colors';
import { NODE_IDS } from '../constants';

interface TaskNodeProps {
  task: Task;
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  expandedNodes: Set<string>;
  toggleNode: (id: string) => void;
  debugMode?: boolean;
}

interface MappingTreeNodeProps {
  mapping: MappingItem;
  index: number;
  task: Task;
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  debugMode: boolean;
}

function MappingTreeNode({ mapping, index, task, workflow, level, selection, onSelect, debugMode }: MappingTreeNodeProps) {
  const debugState = useMappingDebugState(task, workflow, index);

  return (
    <TreeNode
      label={mapping.path}
      icon={<ArrowRightLeft size={14} />}
      iconColor={TREE_COLORS.mapping}
      level={level}
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
      debugState={debugMode ? debugState.state : null}
      isCurrent={debugMode && debugState.isCurrent}
    />
  );
}

interface ValidationRuleTreeNodeProps {
  rule: ValidationRule;
  index: number;
  task: Task;
  workflow: Workflow;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  debugMode: boolean;
}

function ValidationRuleTreeNode({ rule, index, task, workflow, level, selection, onSelect, debugMode }: ValidationRuleTreeNodeProps) {
  const debugState = useValidationRuleDebugState(task, workflow, index);

  return (
    <TreeNode
      label={rule.message}
      icon={<CheckCircle size={14} />}
      iconColor={TREE_COLORS.validation}
      level={level}
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
      debugState={debugMode ? debugState.state : null}
      isCurrent={debugMode && debugState.isCurrent}
    />
  );
}

export function TaskNode({
  task,
  workflow,
  level,
  selection,
  onSelect,
  expandedNodes,
  toggleNode,
  debugMode = false,
}: TaskNodeProps) {
  const taskId = NODE_IDS.task(workflow.id, task.id);
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

  // Get debug states
  const taskDebugState = useTaskDebugState(task, workflow);
  const taskConditionDebugState = useTaskConditionDebugState(task, workflow);

  return (
    <TreeNode
      label={task.name}
      icon={<Box size={14} />}
      iconColor={TREE_COLORS.task}
      isExpanded={isExpanded}
      isSelected={isTaskSelected}
      hasChildren={hasChildren}
      level={level}
      onToggle={() => toggleNode(taskId)}
      onClick={() => onSelect({ type: 'task', task, workflow })}
      debugState={debugMode ? taskDebugState.state : null}
      isCurrent={debugMode && taskDebugState.isCurrent}
    >
      {/* Task Condition */}
      {hasCondition && (
        <TreeNode
          label="Condition"
          icon={<GitBranch size={14} />}
          iconColor={TREE_COLORS.condition}
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
          debugState={debugMode ? taskConditionDebugState.state : null}
          conditionResult={debugMode ? taskConditionDebugState.conditionResult : undefined}
          isCurrent={debugMode && taskConditionDebugState.isCurrent}
        />
      )}

      {/* Mappings for map function - using wrapper component for hook support */}
      {mappings.map((mapping, index) => (
        <MappingTreeNode
          key={`mapping-${index}`}
          mapping={mapping}
          index={index}
          task={task}
          workflow={workflow}
          level={level + 1}
          selection={selection}
          onSelect={onSelect}
          debugMode={debugMode}
        />
      ))}

      {/* Validations for validation function - using wrapper component for hook support */}
      {rules.map((rule, index) => (
        <ValidationRuleTreeNode
          key={`rule-${index}`}
          rule={rule}
          index={index}
          task={task}
          workflow={workflow}
          level={level + 1}
          selection={selection}
          onSelect={onSelect}
          debugMode={debugMode}
        />
      ))}
    </TreeNode>
  );
}
