import dagre from '@dagrejs/dagre';
import type { Node, Edge } from '@xyflow/react';

/**
 * Run a top-to-bottom Dagre layout over `nodes` and `edges`, convert Dagre's
 * center coordinates to React Flow top-left positions, and assign branch
 * handle sides from the resulting geometry.
 */
export function layoutFlowGraph(
  nodes: Node[],
  edges: Edge[],
  nodeWidth: number,
  getNodeHeight: (type: string | undefined) => number,
): void {
  const g = new dagre.graphlib.Graph();
  g.setDefaultEdgeLabel(() => ({}));
  g.setGraph({ rankdir: 'TB', nodesep: 50, ranksep: 80, marginx: 20, marginy: 20 });

  for (const node of nodes) {
    g.setNode(node.id, { width: nodeWidth, height: getNodeHeight(node.type) });
  }
  for (const edge of edges) {
    g.setEdge(edge.source, edge.target);
  }

  dagre.layout(g);

  for (const node of nodes) {
    const pos = g.node(node.id);
    const height = getNodeHeight(node.type);
    node.position = {
      x: pos.x - nodeWidth / 2,
      y: pos.y - height / 2,
    };
  }

  assignBranchSides(nodes, edges);
}

/**
 * Record which side each branching node's `true` edge actually landed on.
 *
 * A branching node (condition / loop guard) exposes two bottom source handles,
 * `true` and `false`. Which side each *should* sit on is not a style constant:
 * dagre decides where the two target nodes end up, and it derives that from
 * graph structure and insertion order. The four places that emit a branch do
 * not push their true/false targets in a consistent order, so a fixed CSS
 * `left:` for each handle matched the layout at two of them and produced
 * crossed edges at the other two.
 *
 * So the handle side is read back off the layout instead. Call this AFTER
 * `dagre.layout()` has written positions; it sets `data.trueOnLeft` on every
 * branching node, which the node components turn into the handle offsets.
 *
 * Nodes whose two branches share a column (or that have only one branch edge)
 * keep the default `true`, matching the historical CSS.
 */
export function assignBranchSides(nodes: Node[], edges: Edge[]): void {
  const xById = new Map(nodes.map((n) => [n.id, n.position.x]));

  for (const node of nodes) {
    if (node.type !== 'condition' && node.type !== 'loopGuard') continue;

    const trueEdge = edges.find(
      (e) => e.source === node.id && e.sourceHandle === 'true'
    );
    const falseEdge = edges.find(
      (e) => e.source === node.id && e.sourceHandle === 'false'
    );
    if (!trueEdge || !falseEdge) continue;

    const trueX = xById.get(trueEdge.target);
    const falseX = xById.get(falseEdge.target);
    if (trueX === undefined || falseX === undefined || trueX === falseX) continue;

    node.data = { ...node.data, trueOnLeft: trueX < falseX };
  }
}

/** Handle offsets across the diamond's width, outermost-first. */
export const BRANCH_HANDLE_LEFT = '15%';
export const BRANCH_HANDLE_RIGHT = '85%';

/** Resolve the two handle offsets from a node's recorded branch side. */
export function branchHandleOffsets(trueOnLeft: boolean | undefined): {
  trueLeft: string;
  falseLeft: string;
} {
  // Default to true-on-left, which is what the CSS pinned before this existed.
  const onLeft = trueOnLeft !== false;
  return {
    trueLeft: onLeft ? BRANCH_HANDLE_LEFT : BRANCH_HANDLE_RIGHT,
    falseLeft: onLeft ? BRANCH_HANDLE_RIGHT : BRANCH_HANDLE_LEFT,
  };
}
