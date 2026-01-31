import dagre from '@dagrejs/dagre';
import type { Node, Edge } from '@xyflow/react';
import type { Workflow } from '../../../../types';

const NODE_WIDTH = 260;
const NODE_HEIGHT_PILL = 40;
const NODE_HEIGHT_DIAMOND = 80;
const NODE_HEIGHT_GROUP = 72;
const NODE_HEIGHT_SKIP = 40;

function hasCondition(condition: unknown): boolean {
  return condition !== undefined && condition !== null && condition !== true;
}

export function buildGroupFlowGraph(workflows: Workflow[]): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];
  let nodeId = 0;
  const id = () => `g${nodeId++}`;

  // Sort workflows by priority
  const sorted = [...workflows].sort((a, b) => (a.priority ?? 0) - (b.priority ?? 0));

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
          taskCount: workflow.tasks.length,
          hasCondition: true,
          workflowId: workflow.id,
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
          taskCount: workflow.tasks.length,
          hasCondition: false,
          workflowId: workflow.id,
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

  // Apply dagre layout
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: 'TB', nodesep: 50, ranksep: 80, marginx: 20, marginy: 20 });

  for (const node of nodes) {
    let height = NODE_HEIGHT_PILL;
    if (node.type === 'condition') height = NODE_HEIGHT_DIAMOND;
    else if (node.type === 'workflowGroup') height = NODE_HEIGHT_GROUP;
    else if (node.type === 'skip') height = NODE_HEIGHT_SKIP;
    g.setNode(node.id, { width: NODE_WIDTH, height });
  }

  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  for (const node of nodes) {
    const pos = g.node(node.id);
    let height = NODE_HEIGHT_PILL;
    if (node.type === 'condition') height = NODE_HEIGHT_DIAMOND;
    else if (node.type === 'workflowGroup') height = NODE_HEIGHT_GROUP;
    else if (node.type === 'skip') height = NODE_HEIGHT_SKIP;
    node.position = {
      x: pos.x - NODE_WIDTH / 2,
      y: pos.y - height / 2,
    };
  }

  return { nodes, edges };
}
