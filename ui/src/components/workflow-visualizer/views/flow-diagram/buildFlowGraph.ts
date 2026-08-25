import dagre from '@dagrejs/dagre';
import { assignBranchSides } from './assignBranchSides';
import type { Node, Edge } from '@xyflow/react';
import type { Workflow, Task, Step, TaskGroup } from '../../../../types';
import { groupMembers, isTaskGroup, loopGuardLabel, loopStepLabel } from '../../../../types';

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
    case 'loopGuard':
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
    terminal: task.terminal,
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

  const loop = workflow.loop;
  let guardId: string | undefined;
  let tailId: string | undefined;

  // ---- Loop guard ----
  // The engine checks `counter < max` before re-evaluating the condition, so
  // the guard sits above it. Its false branch is a loop exit, wired to End
  // once End exists.
  if (loop) {
    guardId = addNode('loopGuard', {
      label: loopGuardLabel(loop),
      variant: 'guard',
    });
    connect(guardId);
    prevNodeId = guardId;
    prevSourceHandle = 'true';
  }

  // ---- Workflow condition ----
  if (hasCondition(workflow.condition)) {
    const condId = addNode('condition', {
      label: 'Workflow\nCondition',
      conditionType: 'workflow',
    });
    // Without a loop this leaves Start and carries no label; with one it
    // leaves the guard's true branch and is labelled like any other.
    connect(condId);

    const skipId = addNode('skip', {});
    edges.push({
      id: `e-${condId}-${skipId}`,
      source: condId,
      target: skipId,
      sourceHandle: 'false',
      // A false condition *breaks* the loop rather than skipping one sweep,
      // so under a loop this branch is an exit, not a per-sweep skip.
      label: loop ? 'Exit loop' : 'No',
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

  /**
   * A group is one condition node gating a whole span: the "Yes" branch runs
   * the member steps in order, the "No" branch bypasses them, and both
   * converge afterwards. That mirrors the engine, which evaluates the group
   * condition once on entry rather than per member.
   */
  const emitGroup = (group: TaskGroup) => {
    if (!hasCondition(group.condition)) {
      for (const child of groupMembers(group)) emitStep(child);
      return;
    }

    const groupCondId = addNode('condition', {
      label: `${group.name ?? group.id}\nGroup Condition`,
      conditionType: 'task',
    });
    connect(groupCondId);

    const bypassId = addNode('skip', {});
    edges.push({
      id: `e-${groupCondId}-${bypassId}`,
      source: groupCondId,
      target: bypassId,
      sourceHandle: 'false',
      label: 'No',
      style: { strokeDasharray: '6 3' },
      className: 'df-flow-edge-false',
    });

    prevNodeId = groupCondId;
    prevSourceHandle = 'true';
    for (const child of groupMembers(group)) emitStep(child);

    const mergeId = addNode('skip', { merge: true });
    connect(mergeId);
    edges.push({ id: `e-${bypassId}-${mergeId}`, source: bypassId, target: mergeId });

    prevNodeId = mergeId;
    prevSourceHandle = undefined;
  };

  const emitStep = (step: Step) => {
    if (isTaskGroup(step)) {
      emitGroup(step);
    } else {
      emitTask(step);
    }
  };

  for (const step of workflow.tasks) {
    emitStep(step);
  }

  // ---- Loop tail ----
  // The counter advance the engine performs after each sweep. It is a sink in
  // the DAG; the back-edge to the guard is added after layout.
  if (loop) {
    tailId = addNode('loopTail', {
      label: loopStepLabel(loop),
      variant: 'tail',
    });
    connect(tailId);
    prevNodeId = tailId;
    prevSourceHandle = undefined;
  }

  // ---- End ----
  const endId = addNode('startEnd', { label: 'End', variant: 'end' });
  if (guardId) {
    // Under a loop the body never falls through to End: the exits are the
    // guard's bound check and the condition going false. Both are normal
    // completion, so both land on End rather than an error node.
    edges.push({
      id: `e-${guardId}-${endId}`,
      source: guardId,
      target: endId,
      sourceHandle: 'false',
      label: 'Reached max',
      className: 'df-flow-edge-loop-exit',
    });
  } else {
    // Explicit rather than `connect`: the End edge carries the pending handle
    // but never the "Yes" label, even when it leaves a condition's true branch.
    edges.push({
      id: `e-${prevNodeId}-${endId}`,
      source: prevNodeId,
      target: endId,
      sourceHandle: prevSourceHandle,
    });
  }
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

  // Handle sides follow the layout dagre just produced, not a fixed CSS rule.
  // Runs before the loop back-edge is pushed: that edge is not a true/false
  // branch, so it cannot affect the sides, and keeping it out keeps the scan
  // over branch edges only.
  assignBranchSides(nodes, edges);

  // ---- Loop back-edge ----
  // Deliberately added after dagre has run: dagre requires a DAG, and feeding
  // it this cycle inverts the ranks and renders the diagram inside-out. Enters
  // and leaves on the left so it sweeps clear of the main column.
  if (guardId && tailId) {
    edges.push({
      id: `e-loop-${tailId}-${guardId}`,
      source: tailId,
      target: guardId,
      sourceHandle: 'loop-out',
      targetHandle: 'loop-in',
      type: 'smoothstep',
      animated: true,
      label: 'repeat',
      className: 'df-flow-edge-loop',
    });
  }

  return { nodes, edges };
}
