import { useMemo, useCallback } from 'react';
import { ReactFlow, Background, Controls, type NodeTypes, type NodeMouseHandler } from '@xyflow/react';
import type { Workflow, JsonLogicValue } from '../../../../types';
import type { TreeSelectionType } from '../../WorkflowVisualizer';
import { useTheme } from '../../context';
import { FlowStartEndNode, FlowConditionNode, FlowSkipNode, FlowWorkflowGroupNode } from './nodes';
import { buildGroupFlowGraph } from './buildGroupFlowGraph';

const nodeTypes: NodeTypes = {
  startEnd: FlowStartEndNode,
  condition: FlowConditionNode,
  skip: FlowSkipNode,
  workflowGroup: FlowWorkflowGroupNode,
};

interface GroupFlowDiagramProps {
  workflows: Workflow[];
  name: string;
  onSelect: (selection: TreeSelectionType) => void;
}

export function GroupFlowDiagram({ workflows, name: _name, onSelect }: GroupFlowDiagramProps) {
  const { resolvedTheme } = useTheme();
  const { nodes, edges } = useMemo(() => buildGroupFlowGraph(workflows), [workflows]);

  const onNodeClick: NodeMouseHandler = useCallback((_event, node) => {
    if (node.type === 'workflowGroup') {
      const workflowId = node.data.workflowId as string;
      const workflow = workflows.find((w: Workflow) => w.id === workflowId);
      if (workflow) {
        onSelect({ type: 'workflow', workflow });
      }
    } else if (node.type === 'condition') {
      const workflowId = node.data.workflowId as string;
      if (workflowId) {
        const workflow = workflows.find((w: Workflow) => w.id === workflowId);
        if (workflow?.condition) {
          onSelect({
            type: 'workflow-condition',
            workflow,
            condition: workflow.condition as JsonLogicValue,
          });
        }
      }
    }
  }, [workflows, onSelect]);

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
