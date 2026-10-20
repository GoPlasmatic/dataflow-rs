import { describe, expect, it } from 'vitest';
import { fireEvent, render, waitFor } from '@testing-library/react';
import { WorkflowVisualizer, type VisualizerSelection } from './WorkflowVisualizer';
import type { Workflow } from '../../types';

const WORKFLOWS: Workflow[] = [
  {
    id: 'w1',
    name: 'Order Intake',
    priority: 1,
    loop: {
      max: 2,
      setup: [
        {
          id: 'setup_task',
          name: 'Setup Task',
          function: { name: 'log', input: { message: 'setup' } },
        },
      ],
    },
    tasks: [
      {
        id: 't1',
        name: 'Parse Order',
        function: { name: 'parse_json', input: { source: 'payload', target: 'order' } },
      },
      {
        id: 'g1',
        name: 'Validation Group',
        tasks: [
          {
            id: 'grouped_task',
            name: 'Grouped Task',
            function: { name: 'map', input: { mappings: [{ path: 'data.ok', logic: true }] } },
          },
        ],
      },
    ],
  },
  {
    id: 'w2',
    name: 'Second Workflow',
    priority: 2,
    path: 'orders/billing',
    tasks: [
      {
        id: 't2',
        name: 'Bill Order',
        function: { name: 'log', input: { message: 'billed' } },
      },
    ],
  },
];

describe('WorkflowVisualizer selection props', () => {
  it('starts with nothing selected by default', () => {
    const { container } = render(<WorkflowVisualizer workflows={WORKFLOWS} />);
    expect(
      container.querySelector('.df-details-empty')?.textContent,
    ).toContain('Select an item from the explorer to view its details and logic visualization');
  });

  it('opens a workflow flow view on first render via initialSelection={{ kind: "workflow", workflowId }}', async () => {
    const { container } = render(
      <WorkflowVisualizer
        workflows={WORKFLOWS}
        initialSelection={{ kind: 'workflow', workflowId: 'w1' }}
      />,
    );
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Order Intake');
    expect(container.querySelector('.df-details-header-subtitle')?.textContent).toBe('Workflow Flow');
    await waitFor(() => expect(container.querySelector('.react-flow')).not.toBeNull());
  });

  it('supports the "first-workflow" initialSelection shorthand', async () => {
    const { container } = render(
      <WorkflowVisualizer workflows={WORKFLOWS} initialSelection="first-workflow" />,
    );
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Order Intake');
    await waitFor(() => expect(container.querySelector('.react-flow')).not.toBeNull());
  });

  it('opens a task inside a group and inside loop.setup via initialSelection={{ kind: "task", ... }}', () => {
    const { container, unmount } = render(
      <WorkflowVisualizer
        workflows={WORKFLOWS}
        initialSelection={{ kind: 'task', workflowId: 'w1', taskId: 'grouped_task' }}
      />,
    );
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Grouped Task');
    expect(container.querySelector('.df-details-header-subtitle')?.textContent).toBe('Order Intake / map');
    unmount();

    const { container: setupContainer } = render(
      <WorkflowVisualizer
        workflows={WORKFLOWS}
        initialSelection={{ kind: 'task', workflowId: 'w1', taskId: 'setup_task' }}
      />,
    );
    expect(setupContainer.querySelector('.df-details-header-title')?.textContent).toBe('Setup Task');
    expect(setupContainer.querySelector('.df-details-header-subtitle')?.textContent).toBe('Order Intake / log');
  });

  it('supports group selection by opening its first leaf task', () => {
    const { container } = render(
      <WorkflowVisualizer
        workflows={WORKFLOWS}
        initialSelection={{ kind: 'group', workflowId: 'w1', groupId: 'g1' }}
      />,
    );
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Grouped Task');
  });

  it('keeps explorer and detail panel in sync in controlled mode and reports changes via onSelectionChange', () => {
    const changes: Array<VisualizerSelection | null> = [];
    const { container, rerender } = render(
      <WorkflowVisualizer
        workflows={WORKFLOWS}
        selection={{ kind: 'task', workflowId: 'w2', taskId: 't2' }}
        onSelectionChange={(sel) => changes.push(sel)}
      />,
    );

    // Controlled selection opens w2 / t2 and expands its parent folders
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Bill Order');
    const selectedLabels = Array.from(
      container.querySelectorAll('.df-tree-node-selected > .df-tree-label'),
    ).map((n) => n.textContent);
    expect(selectedLabels).toEqual(['Bill Order']);

    // Clicking another item in the explorer fires onSelectionChange without mutating controlled view until prop updates
    const parseOrderNode = Array.from(container.querySelectorAll('.df-tree-label')).find(
      (n) => n.textContent === 'Parse Order',
    )!;
    fireEvent.click(parseOrderNode);
    expect(changes).toEqual([{ kind: 'task', workflowId: 'w1', taskId: 't1' }]);
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Bill Order');

    // Updating the controlled prop updates both explorer and detail panel
    rerender(
      <WorkflowVisualizer
        workflows={WORKFLOWS}
        selection={changes[0]}
        onSelectionChange={(sel) => changes.push(sel)}
      />,
    );
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Parse Order');

    // Passing selection={null} clears the selection
    rerender(
      <WorkflowVisualizer
        workflows={WORKFLOWS}
        selection={null}
        onSelectionChange={(sel) => changes.push(sel)}
      />,
    );
    expect(
      container.querySelector('.df-details-empty')?.textContent,
    ).toContain('Select an item from the explorer to view its details and logic visualization');
  });

  it('reconciles uncontrolled selection when workflows prop is updated', () => {
    const { container, rerender } = render(
      <WorkflowVisualizer
        workflows={WORKFLOWS}
        initialSelection={{ kind: 'task', workflowId: 'w2', taskId: 't2' }}
      />,
    );
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Bill Order');

    const updated = WORKFLOWS.map((w) =>
      w.id === 'w2'
        ? {
            ...w,
            tasks: [{ id: 't2', name: 'Renamed Bill Task', function: { name: 'log', input: { message: 'billed' } } }],
          }
        : w,
    );
    rerender(<WorkflowVisualizer workflows={updated} />);
    expect(container.querySelector('.df-details-header-title')?.textContent).toBe('Renamed Bill Task');
  });
});
