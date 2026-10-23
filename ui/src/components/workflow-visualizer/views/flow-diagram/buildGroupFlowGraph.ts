import { layoutFlowGraph } from './assignBranchSides';
import type { Node, Edge } from '@xyflow/react';
import { sortWorkflowsByPriority, workflowTasks, hasCondition, type Workflow } from '../../../../types';

const NODE_WIDTH = 260;
const NODE_HEIGHT_PILL = 40;
const NODE_HEIGHT_DIAMOND = 80;
const NODE_HEIGHT_GROUP = 72;
const NODE_HEIGHT_SKIP = 40;

function nodeHeight(type: string | undefined): number {
  if (type === 'condition') return NODE_HEIGHT_DIAMOND;
  if (type === 'workflowGroup') return NODE_HEIGHT_GROUP;
  if (type === 'skip') return NODE_HEIGHT_SKIP;
  return NODE_HEIGHT_PILL;
}

export function buildGroupFlowGraph(workflows: Workflow[]): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];
  let nodeId = 0;
  const id = () => `g${nodeId++}`;

  // Sort workflows by priority
  const sorted = sortWorkflowsByPriority(workflows);

  // Start node
  const startId = id();
  nodes.push({
    id: startId,
    type: 'startEnd',
    position: { x: 0, y: 0 },
    data: { label: 'Message In', variant: 'start' },
    draggable: false,
    connectable: false,
  });

  let prevNodeId = startId;
  let prevSourceHandle: string | undefined;

  for (const workflow of sorted) {
    if (hasCondition(workflow.condition)) {
      // Condition diamond
      const condId = id();
      nodes.push({
        id: condId,
        type: 'condition',
        position: { x: 0, y: 0 },
        data: {
          label: `${workflow.name}\nCondition`,
          conditionType: 'workflow',
          workflowId: workflow.id,
        },
        draggable: false,
        connectable: false,
      });
      edges.push({
        id: `e-${prevNodeId}-${condId}`,
        source: prevNodeId,
        target: condId,
        sourceHandle: prevSourceHandle,
        label: prevSourceHandle === 'true' ? 'Yes' : undefined,
        className: prevSourceHandle === 'true' ? 'df-flow-edge-true' : undefined,
      });

      // Workflow group node on true branch
      const groupId = id();
      nodes.push({
        id: groupId,
        type: 'workflowGroup',
        position: { x: 0, y: 0 },
        data: {
          workflowName: workflow.name,
          priority: workflow.priority,
          taskCount: workflowTasks(workflow).length,
          hasCondition: true,
          workflowId: workflow.id,
          loop: workflow.loop,
        },
        draggable: false,
        connectable: false,
      });
      edges.push({
        id: `e-${condId}-${groupId}`,
        source: condId,
        target: groupId,
        sourceHandle: 'true',
        label: 'Yes',
        className: 'df-flow-edge-true',
      });

      // Skip node on false branch
      const skipId = id();
      nodes.push({
        id: skipId,
        type: 'skip',
        position: { x: 0, y: 0 },
        data: {},
        draggable: false,
        connectable: false,
      });
      edges.push({
        id: `e-${condId}-${skipId}`,
        source: condId,
        target: skipId,
        sourceHandle: 'false',
        label: 'No',
        style: { strokeDasharray: '6 3' },
        className: 'df-flow-edge-false',
      });

      // Merge point
      const mergeId = id();
      nodes.push({
        id: mergeId,
        type: 'skip',
        position: { x: 0, y: 0 },
        data: { merge: true },
        draggable: false,
        connectable: false,
      });
      edges.push(
        { id: `e-${groupId}-${mergeId}`, source: groupId, target: mergeId },
        { id: `e-${skipId}-${mergeId}`, source: skipId, target: mergeId },
      );

      prevNodeId = mergeId;
      prevSourceHandle = undefined;
    } else {
      // Unconditional workflow — just a group node
      const groupId = id();
      nodes.push({
        id: groupId,
        type: 'workflowGroup',
        position: { x: 0, y: 0 },
        data: {
          workflowName: workflow.name,
          priority: workflow.priority,
          taskCount: workflowTasks(workflow).length,
          hasCondition: false,
          workflowId: workflow.id,
          loop: workflow.loop,
        },
        draggable: false,
        connectable: false,
      });
      edges.push({
        id: `e-${prevNodeId}-${groupId}`,
        source: prevNodeId,
        target: groupId,
        sourceHandle: prevSourceHandle,
        label: prevSourceHandle === 'true' ? 'Yes' : undefined,
        className: prevSourceHandle === 'true' ? 'df-flow-edge-true' : undefined,
      });
      prevNodeId = groupId;
      prevSourceHandle = undefined;
    }
  }

  // End node
  const endId = id();
  nodes.push({
    id: endId,
    type: 'startEnd',
    position: { x: 0, y: 0 },
    data: { label: 'Done', variant: 'end' },
    draggable: false,
    connectable: false,
  });
  edges.push({
    id: `e-${prevNodeId}-${endId}`,
    source: prevNodeId,
    target: endId,
    sourceHandle: prevSourceHandle,
  });

  // Apply dagre layout and assign branch sides
  layoutFlowGraph(nodes, edges, NODE_WIDTH, nodeHeight);

  return { nodes, edges };
}
