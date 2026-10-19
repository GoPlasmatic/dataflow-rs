import { describe, expect, it } from 'vitest';
import { buildFlowGraph } from './buildFlowGraph';
import type { Workflow } from '../../../../types';

describe('buildFlowGraph', () => {
  it('builds a linear start -> task -> end graph for an unconditional workflow', () => {
    const workflow: Workflow = {
      id: 'w1',
      name: 'Linear Workflow',
      tasks: [
        {
          id: 't1',
          name: 'First Task',
          function: { name: 'map', input: { mappings: [] } },
        },
        {
          id: 't2',
          name: 'Second Task',
          function: { name: 'log', input: {} },
          on_failure: 'continue',
          terminal: true,
        },
      ],
    };

    const { nodes, edges } = buildFlowGraph(workflow);
    expect(nodes.map((n) => n.type)).toEqual(['startEnd', 'task', 'task', 'startEnd']);
    expect(nodes[0].data).toMatchObject({ label: 'Start', variant: 'start' });
    expect(nodes[1].data).toMatchObject({
      taskId: 't1',
      taskName: 'First Task',
      functionName: 'map',
      continuesOnFailure: false,
    });
    expect(nodes[2].data).toMatchObject({
      taskId: 't2',
      taskName: 'Second Task',
      functionName: 'log',
      continuesOnFailure: true,
      terminal: true,
    });
    expect(nodes[3].data).toMatchObject({ label: 'End', variant: 'end' });
    expect(edges.map((e) => [e.source, e.target])).toEqual([
      ['n0', 'n1'],
      ['n1', 'n2'],
      ['n2', 'n3'],
    ]);
  });

  it('adds condition, skip, and merge nodes for conditional workflows, tasks, and groups', () => {
    const workflow: Workflow = {
      id: 'w_cond',
      name: 'Conditional Workflow',
      condition: { '==': [{ var: 'data.active' }, true] },
      tasks: [
        {
          id: 't_cond',
          name: 'Guarded Task',
          condition: { '!!': [{ var: 'data.ready' }] },
          function: { name: 'map', input: { mappings: [] } },
        },
        {
          id: 'g1',
          name: 'Guarded Group',
          condition: { '==': [{ var: 'data.group' }, 1] },
          tasks: [
            {
              id: 't_in_group',
              name: 'Group Member',
              function: { name: 'log', input: {} },
            },
          ],
        },
      ],
    };

    const { nodes, edges } = buildFlowGraph(workflow);
    const types = nodes.map((n) => n.type);
    expect(types.filter((t) => t === 'condition')).toHaveLength(3);
    expect(types.filter((t) => t === 'task')).toHaveLength(2);
    expect(edges.some((e) => e.label === 'Yes')).toBe(true);
    expect(edges.some((e) => e.label === 'No')).toBe(true);
  });

  it('adds loop setup, loopGuard, loopTail, and a repeat back-edge for a looping workflow', () => {
    const workflow: Workflow = {
      id: 'w_loop',
      name: 'Looping Workflow',
      condition: { '<': [{ var: 'temp_data.i' }, 3] },
      loop: {
        counter: 'i',
        max: 5,
        over: { var: 'data.items' },
        as: 'item',
        setup: [
          {
            id: 'init',
            name: 'Init',
            function: { name: 'map', input: { mappings: [] } },
          },
        ],
      },
      tasks: [
        {
          id: 'body',
          name: 'Body',
          function: { name: 'map', input: { mappings: [] } },
        },
      ],
    };

    const { nodes, edges } = buildFlowGraph(workflow);
    expect(nodes.some((n) => n.type === 'loopGuard')).toBe(true);
    expect(nodes.some((n) => n.type === 'loopTail')).toBe(true);
    const repeatEdge = edges.find((e) => e.label === 'repeat');
    expect(repeatEdge).toBeDefined();
    expect(repeatEdge?.animated).toBe(true);
    expect(edges.some((e) => e.label === 'Exit loop')).toBe(true);
    expect(edges.some((e) => e.label === 'Done')).toBe(true);
  });
});
