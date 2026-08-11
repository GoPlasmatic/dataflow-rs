import dagre from '@dagrejs/dagre';
import type { Node, Edge } from '@xyflow/react';
import type { Workflow, Task } from '../../../../types';

const NODE_WIDTH = 200;
const NODE_HEIGHT_PILL = 40;
const NODE_HEIGHT_DIAMOND = 80;
const NODE_HEIGHT_TASK = 72;
const NODE_HEIGHT_SKIP = 40;

function hasCondition(condition: unknown): boolean {
  return condition !== undefined && condition !== null && condition !== true;
}

/** Layout height for a node type. Used twice — before and after dagre runs. */
function nodeHeight(type: string | undefined): number {
  switch (type) {
    case 'condition':
      return NODE_HEIGHT_DIAMOND;
    case 'task':
      return NODE_HEIGHT_TASK;
    case 'skip':
      return NODE_HEIGHT_SKIP;
    default:
      return NODE_HEIGHT_PILL;
  }
}

export function buildFlowGraph(workflow: Workflow): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];
  let nodeId = 0;
  const id = () => `n${nodeId++}`;

  const addNode = (type: string, data: Record<string, unknown>): string => {
    const newId = id();
    nodes.push({
      id: newId,
      type,
      position: { x: 0, y: 0 },
      data,
      draggable: false,
      connectable: false,
    });
    return newId;
  };

  const taskData = (task: Task) => ({
    taskName: task.name,
    functionName: task.function.name,
    description: task.description,
    continueOnError: task.continue_on_error,
    taskId: task.id,
    workflowId: workflow.id,
  });

  // Cursor: where the next forward edge originates. A pending 'true' handle
  // carries the "Yes" label onto whatever edge is drawn next.
  let prevNodeId = addNode('startEnd', { label: 'Start', variant: 'start' });
  let prevSourceHandle: string | undefined;

  /** Draw an edge from the cursor to `target`, carrying the pending label. */
  const connect = (target: string) => {
    edges.push({
      id: `e-${prevNodeId}-${target}`,
      source: prevNodeId,
      target,
      sourceHandle: prevSourceHandle,
      label: prevSourceHandle === 'true' ? 'Yes' : undefined,
      className: prevSourceHandle === 'true' ? 'df-flow-edge-true' : undefined,
    });
  };

  // Skip nodes discovered before the End node exists.
  const danglingToEnd: string[] = [];

  // ---- Workflow condition ----
  if (hasCondition(workflow.condition)) {
    const condId = addNode('condition', {
      label: 'Workflow\nCondition',
      conditionType: 'workflow',
    });
    // Explicit rather than `connect`: this edge leaves Start, so it never
    // carries a "Yes" label, and it declares an explicit default edge type.
    edges.push({
      id: `e-${prevNodeId}-${condId}`,
      source: prevNodeId,
      target: condId,
      sourceHandle: prevSourceHandle,
      type: 'default',
    });

    const skipId = addNode('skip', {});
    edges.push({
      id: `e-${condId}-${skipId}`,
      source: condId,
      target: skipId,
      sourceHandle: 'false',
      label: 'No',
      style: { strokeDasharray: '6 3' },
      className: 'df-flow-edge-false',
    });
    danglingToEnd.push(skipId);

    prevNodeId = condId;
    prevSourceHandle = 'true';
  }

  // ---- Tasks ----
  const emitTask = (task: Task) => {
    if (!hasCondition(task.condition)) {
      const taskNodeId = addNode('task', taskData(task));
      connect(taskNodeId);
      prevNodeId = taskNodeId;
      prevSourceHandle = undefined;
      return;
    }

    const taskCondId = addNode('condition', {
      label: `${task.name}\nCondition`,
      conditionType: 'task',
    });
    connect(taskCondId);

    const taskNodeId = addNode('task', taskData(task));
    edges.push({
      id: `e-${taskCondId}-${taskNodeId}`,
      source: taskCondId,
      target: taskNodeId,
      sourceHandle: 'true',
      label: 'Yes',
      className: 'df-flow-edge-true',
    });

    const taskSkipId = addNode('skip', {});
    edges.push({
      id: `e-${taskCondId}-${taskSkipId}`,
      source: taskCondId,
      target: taskSkipId,
      sourceHandle: 'false',
      label: 'No',
      style: { strokeDasharray: '6 3' },
      className: 'df-flow-edge-false',
    });

    // Both branches converge so the next task has a single predecessor.
    const mergeId = addNode('skip', { merge: true });
    edges.push(
      { id: `e-${taskNodeId}-${mergeId}`, source: taskNodeId, target: mergeId },
      { id: `e-${taskSkipId}-${mergeId}`, source: taskSkipId, target: mergeId },
    );

    prevNodeId = mergeId;
    prevSourceHandle = undefined;
  };

  for (const task of workflow.tasks) {
    emitTask(task);
  }

  // ---- End ----
  const endId = addNode('startEnd', { label: 'End', variant: 'end' });
  // Explicit rather than `connect`: the End edge carries the pending handle
  // but never the "Yes" label, even when it leaves a condition's true branch.
  edges.push({
    id: `e-${prevNodeId}-${endId}`,
    source: prevNodeId,
    target: endId,
    sourceHandle: prevSourceHandle,
  });
  for (const danglingId of danglingToEnd) {
    edges.push({ id: `e-${danglingId}-${endId}`, source: danglingId, target: endId });
  }

  // ---- Layout ----
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: 'TB', nodesep: 50, ranksep: 80, marginx: 20, marginy: 20 });

  for (const node of nodes) {
    g.setNode(node.id, { width: NODE_WIDTH, height: nodeHeight(node.type) });
  }
  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  for (const node of nodes) {
    const pos = g.node(node.id);
    node.position = {
      x: pos.x - NODE_WIDTH / 2,
      y: pos.y - nodeHeight(node.type) / 2,
    };
  }

  return { nodes, edges };
}
