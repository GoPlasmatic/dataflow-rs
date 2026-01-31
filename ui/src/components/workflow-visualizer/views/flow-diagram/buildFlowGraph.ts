import dagre from '@dagrejs/dagre';
import type { Node, Edge } from '@xyflow/react';
import type { Workflow } from '../../../../types';

const NODE_WIDTH = 200;
const NODE_HEIGHT_PILL = 40;
const NODE_HEIGHT_DIAMOND = 80;
const NODE_HEIGHT_TASK = 72;
const NODE_HEIGHT_SKIP = 40;

function hasCondition(condition: unknown): boolean {
  return condition !== undefined && condition !== null && condition !== true;
}

export function buildFlowGraph(workflow: Workflow): { nodes: Node[]; edges: Edge[] } {
  const nodes: Node[] = [];
  const edges: Edge[] = [];
  let nodeId = 0;
  const id = () => `n${nodeId++}`;

  // Start node
  const startId = id();
  nodes.push({
    id: startId,
    type: 'startEnd',
    position: { x: 0, y: 0 },
    data: { label: 'Start', variant: 'start' },
    draggable: false,
    connectable: false,
  });

  let prevNodeId = startId;
  let prevSourceHandle: string | undefined;

  // Workflow condition
  if (hasCondition(workflow.condition)) {
    const condId = id();
    nodes.push({
      id: condId,
      type: 'condition',
      position: { x: 0, y: 0 },
      data: { label: 'Workflow\nCondition', conditionType: 'workflow' },
      draggable: false,
      connectable: false,
    });
    edges.push({
      id: `e-${prevNodeId}-${condId}`,
      source: prevNodeId,
      target: condId,
      sourceHandle: prevSourceHandle,
      type: 'default',
    });

    // False branch → skip to end
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
    // skipId will connect to end node later — tracked as a "dangling false"
    // We'll collect these and connect them at the end
    const danglingFalseNodes: string[] = [skipId];

    prevNodeId = condId;
    prevSourceHandle = 'true';

    // Add tasks on the true branch
    for (let i = 0; i < workflow.tasks.length; i++) {
      const task = workflow.tasks[i];

      if (hasCondition(task.condition)) {
        // Task condition diamond
        const taskCondId = id();
        nodes.push({
          id: taskCondId,
          type: 'condition',
          position: { x: 0, y: 0 },
          data: { label: `${task.name}\nCondition`, conditionType: 'task' },
          draggable: false,
          connectable: false,
        });
        edges.push({
          id: `e-${prevNodeId}-${taskCondId}`,
          source: prevNodeId,
          target: taskCondId,
          sourceHandle: prevSourceHandle,
          label: prevSourceHandle === 'true' ? 'Yes' : undefined,
          className: prevSourceHandle === 'true' ? 'df-flow-edge-true' : undefined,
        });

        // Task node on true branch
        const taskNodeId = id();
        nodes.push({
          id: taskNodeId,
          type: 'task',
          position: { x: 0, y: 0 },
          data: {
            taskName: task.name,
            functionName: task.function.name,
            description: task.description,
            continueOnError: task.continue_on_error,
            taskId: task.id,
            workflowId: workflow.id,
          },
          draggable: false,
          connectable: false,
        });
        edges.push({
          id: `e-${taskCondId}-${taskNodeId}`,
          source: taskCondId,
          target: taskNodeId,
          sourceHandle: 'true',
          label: 'Yes',
          className: 'df-flow-edge-true',
        });

        // False branch: skip node that merges back
        const taskSkipId = id();
        nodes.push({
          id: taskSkipId,
          type: 'skip',
          position: { x: 0, y: 0 },
          data: {},
          draggable: false,
          connectable: false,
        });
        edges.push({
          id: `e-${taskCondId}-${taskSkipId}`,
          source: taskCondId,
          target: taskSkipId,
          sourceHandle: 'false',
          label: 'No',
          style: { strokeDasharray: '6 3' },
          className: 'df-flow-edge-false',
        });

        // Merge node: both true and false branches converge to next
        // We'll use a virtual merge point — just connect both to the next node
        // For now, track both as previous
        // Use a merge approach: create a merge connector
        const mergeId = id();
        // Use a skip-style merge point (invisible)
        nodes.push({
          id: mergeId,
          type: 'skip',
          position: { x: 0, y: 0 },
          data: { merge: true },
          draggable: false,
          connectable: false,
        });
        edges.push(
          { id: `e-${taskNodeId}-${mergeId}`, source: taskNodeId, target: mergeId },
          { id: `e-${taskSkipId}-${mergeId}`, source: taskSkipId, target: mergeId },
        );

        prevNodeId = mergeId;
        prevSourceHandle = undefined;
      } else {
        // Simple task node, no condition
        const taskNodeId = id();
        nodes.push({
          id: taskNodeId,
          type: 'task',
          position: { x: 0, y: 0 },
          data: {
            taskName: task.name,
            functionName: task.function.name,
            description: task.description,
            continueOnError: task.continue_on_error,
            taskId: task.id,
            workflowId: workflow.id,
          },
          draggable: false,
          connectable: false,
        });
        edges.push({
          id: `e-${prevNodeId}-${taskNodeId}`,
          source: prevNodeId,
          target: taskNodeId,
          sourceHandle: prevSourceHandle,
          label: prevSourceHandle === 'true' ? 'Yes' : undefined,
          className: prevSourceHandle === 'true' ? 'df-flow-edge-true' : undefined,
        });
        prevNodeId = taskNodeId;
        prevSourceHandle = undefined;
      }
    }

    // End node
    const endId = id();
    nodes.push({
      id: endId,
      type: 'startEnd',
      position: { x: 0, y: 0 },
      data: { label: 'End', variant: 'end' },
      draggable: false,
      connectable: false,
    });
    edges.push({
      id: `e-${prevNodeId}-${endId}`,
      source: prevNodeId,
      target: endId,
      sourceHandle: prevSourceHandle,
    });
    // Connect dangling false nodes to end
    for (const falseNodeId of danglingFalseNodes) {
      edges.push({
        id: `e-${falseNodeId}-${endId}`,
        source: falseNodeId,
        target: endId,
      });
    }
  } else {
    // No workflow condition — straightforward sequential
    for (let i = 0; i < workflow.tasks.length; i++) {
      const task = workflow.tasks[i];

      if (hasCondition(task.condition)) {
        const taskCondId = id();
        nodes.push({
          id: taskCondId,
          type: 'condition',
          position: { x: 0, y: 0 },
          data: { label: `${task.name}\nCondition`, conditionType: 'task' },
          draggable: false,
          connectable: false,
        });
        edges.push({
          id: `e-${prevNodeId}-${taskCondId}`,
          source: prevNodeId,
          target: taskCondId,
          sourceHandle: prevSourceHandle,
          label: prevSourceHandle === 'true' ? 'Yes' : undefined,
          className: prevSourceHandle === 'true' ? 'df-flow-edge-true' : undefined,
        });

        const taskNodeId = id();
        nodes.push({
          id: taskNodeId,
          type: 'task',
          position: { x: 0, y: 0 },
          data: {
            taskName: task.name,
            functionName: task.function.name,
            description: task.description,
            continueOnError: task.continue_on_error,
            taskId: task.id,
            workflowId: workflow.id,
          },
          draggable: false,
          connectable: false,
        });
        edges.push({
          id: `e-${taskCondId}-${taskNodeId}`,
          source: taskCondId,
          target: taskNodeId,
          sourceHandle: 'true',
          label: 'Yes',
          className: 'df-flow-edge-true',
        });

        const taskSkipId = id();
        nodes.push({
          id: taskSkipId,
          type: 'skip',
          position: { x: 0, y: 0 },
          data: {},
          draggable: false,
          connectable: false,
        });
        edges.push({
          id: `e-${taskCondId}-${taskSkipId}`,
          source: taskCondId,
          target: taskSkipId,
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
          { id: `e-${taskNodeId}-${mergeId}`, source: taskNodeId, target: mergeId },
          { id: `e-${taskSkipId}-${mergeId}`, source: taskSkipId, target: mergeId },
        );

        prevNodeId = mergeId;
        prevSourceHandle = undefined;
      } else {
        const taskNodeId = id();
        nodes.push({
          id: taskNodeId,
          type: 'task',
          position: { x: 0, y: 0 },
          data: {
            taskName: task.name,
            functionName: task.function.name,
            description: task.description,
            continueOnError: task.continue_on_error,
            taskId: task.id,
            workflowId: workflow.id,
          },
          draggable: false,
          connectable: false,
        });
        edges.push({
          id: `e-${prevNodeId}-${taskNodeId}`,
          source: prevNodeId,
          target: taskNodeId,
          sourceHandle: prevSourceHandle,
          label: prevSourceHandle === 'true' ? 'Yes' : undefined,
          className: prevSourceHandle === 'true' ? 'df-flow-edge-true' : undefined,
        });
        prevNodeId = taskNodeId;
        prevSourceHandle = undefined;
      }
    }

    // End node
    const endId = id();
    nodes.push({
      id: endId,
      type: 'startEnd',
      position: { x: 0, y: 0 },
      data: { label: 'End', variant: 'end' },
      draggable: false,
      connectable: false,
    });
    edges.push({
      id: `e-${prevNodeId}-${endId}`,
      source: prevNodeId,
      target: endId,
      sourceHandle: prevSourceHandle,
    });
  }

  // Apply dagre layout
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: 'TB', nodesep: 50, ranksep: 80, marginx: 20, marginy: 20 });

  for (const node of nodes) {
    let height = NODE_HEIGHT_PILL;
    if (node.type === 'condition') height = NODE_HEIGHT_DIAMOND;
    else if (node.type === 'task') height = NODE_HEIGHT_TASK;
    else if (node.type === 'skip') height = NODE_HEIGHT_SKIP;
    g.setNode(node.id, { width: NODE_WIDTH, height });
  }

  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  // Map computed positions back
  for (const node of nodes) {
    const pos = g.node(node.id);
    let height = NODE_HEIGHT_PILL;
    if (node.type === 'condition') height = NODE_HEIGHT_DIAMOND;
    else if (node.type === 'task') height = NODE_HEIGHT_TASK;
    else if (node.type === 'skip') height = NODE_HEIGHT_SKIP;
    node.position = {
      x: pos.x - NODE_WIDTH / 2,
      y: pos.y - height / 2,
    };
  }

  return { nodes, edges };
}
