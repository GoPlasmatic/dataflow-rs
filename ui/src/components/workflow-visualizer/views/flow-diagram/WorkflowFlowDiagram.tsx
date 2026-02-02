import { useMemo, useCallback } from 'react';
import { ReactFlow, Background, Controls, type NodeTypes, type NodeMouseHandler } from '@xyflow/react';
import type { Workflow, Task, JsonLogicValue } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme } from '../../context';
import { FlowStartEndNode, FlowConditionNode, FlowTaskNode, FlowSkipNode } from './nodes';
import { buildFlowGraph } from './buildFlowGraph';

const nodeTypes: NodeTypes = {
  startEnd: FlowStartEndNode,
  condition: FlowConditionNode,
  task: FlowTaskNode,
  skip: FlowSkipNode,
};

interface WorkflowFlowDiagramProps {
  workflow: Workflow;
  onSelect: (selection: TreeSelectionType) => void;
}

export function WorkflowFlowDiagram({ workflow, onSelect }: WorkflowFlowDiagramProps) {
  const { resolvedTheme } = useTheme();
  const { nodes, edges } = useMemo(() => buildFlowGraph(workflow), [workflow]);

  const onNodeClick: NodeMouseHandler = useCallback((_event, node) => {
    if (node.type === 'task') {
      const taskId = node.data.taskId as string;
      const task = workflow.tasks.find((t: Task) => t.id === taskId);
      if (task) {
        onSelect({ type: 'task', task, workflow });
      }
    } else if (node.type === 'condition') {
      const conditionType = node.data.conditionType as string;
      if (conditionType === 'workflow' && workflow.condition) {
        onSelect({
          type: 'workflow-condition',
          workflow,
          condition: workflow.condition as JsonLogicValue,
        });
      } else if (conditionType === 'task') {
        // Find the task by matching the label
        const label = (node.data.label as string) ?? '';
        const taskName = label.replace('\nCondition', '');
        const task = workflow.tasks.find((t: Task) => t.name === taskName);
        if (task?.condition) {
          onSelect({
            type: 'task-condition',
            task,
            workflow,
            condition: task.condition as JsonLogicValue,
          });
        }
      }
    }
  }, [workflow, onSelect]);

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor" data-theme={resolvedTheme}>
        <div className="df-flow-diagram" data-theme={resolvedTheme}>
          <ReactFlow
            nodes={nodes}
            edges={edges}
            nodeTypes={nodeTypes}
            onNodeClick={onNodeClick}
            colorMode={resolvedTheme}
            fitView
            fitViewOptions={{ padding: 0.2, maxZoom: 0.75 }}
            nodesDraggable={false}
            nodesConnectable={false}
            elementsSelectable={false}
            panOnDrag
            zoomOnScroll
            minZoom={0.3}
            maxZoom={2}
            proOptions={{ hideAttribution: true }}
          >
            <Background gap={20} size={1} />
            <Controls showInteractive={false} />
          </ReactFlow>
        </div>
      </div>
    </div>
  );
}
