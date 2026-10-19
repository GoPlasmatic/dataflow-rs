import { describe, expect, it } from 'vitest';
import { buildGroupFlowGraph } from './buildGroupFlowGraph';
import type { Workflow } from '../../../../types';

describe('buildGroupFlowGraph', () => {
  it('orders workflows by priority and wires unconditional and conditional workflows between Message In and Message Out', () => {
    const workflows: Workflow[] = [
      {
        id: 'second',
        name: 'Second Workflow',
        priority: 20,
        condition: { '==': [{ var: 'data.run_second' }, true] },
        tasks: [
          {
            id: 't2',
            name: 'Task 2',
            function: { name: 'log', input: {} },
          },
        ],
      },
      {
        id: 'first',
        name: 'First Workflow',
        priority: 10,
        tasks: [
          {
            id: 't1',
            name: 'Task 1',
            function: { name: 'map', input: { mappings: [] } },
          },
        ],
      },
    ];

    const { nodes, edges } = buildGroupFlowGraph(workflows);
    const groupNodes = nodes.filter((n) => n.type === 'workflowGroup');
    expect(groupNodes.map((n) => n.data.workflowId)).toEqual(['first', 'second']);
    expect(nodes[0].data).toMatchObject({ label: 'Message In', variant: 'start' });
    expect(nodes[nodes.length - 1].data).toMatchObject({
      label: 'Done',
      variant: 'end',
    });
    expect(nodes.some((n) => n.type === 'condition' && n.data.workflowId === 'second')).toBe(
      true
    );
    expect(edges.some((e) => e.label === 'Yes')).toBe(true);
    expect(edges.some((e) => e.label === 'No')).toBe(true);
  });
});
