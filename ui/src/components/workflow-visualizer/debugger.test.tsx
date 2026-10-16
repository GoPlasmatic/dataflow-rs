import { describe, expect, it } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { WorkflowVisualizer } from './WorkflowVisualizer';
import { defaultEngineFactory } from '../../engines';
import type { Workflow } from '../../types';

/**
 * The first task writes a string, the second appends to it, which fails and
 * stops the run; the third never runs.
 */
const FAILING: Workflow[] = [
  {
    id: 'w',
    name: 'Failing',
    tasks: [
      { id: 'ok', name: 'Write', function: { name: 'map', input: { mappings: [{ path: 'data.s', logic: 'str' }] } } },
      { id: 'f', name: 'Append', function: { name: 'map', input: { mappings: [{ path: 'data.s', logic: 'x', mode: 'append' }] } } },
      { id: 'never', name: 'Never', function: { name: 'map', input: { mappings: [{ path: 'data.z', logic: 1 }] } } },
    ],
  },
];

describe('the debugger', () => {
  it('renders a failing run: the status, and the failed task as an error at the last step', async () => {
    const { container } = render(
      <WorkflowVisualizer
        workflows={FAILING}
        debugConfig={{
          enabled: true,
          engineFactory: defaultEngineFactory,
          initialPayload: {},
          autoExecute: true,
        }}
      />,
    );

    // The run settles and the toolbar reports how it ended.
    const status = await waitFor(() => {
      const element = container.querySelector('.df-debug-toolbar-run-status');
      expect(element).not.toBeNull();
      return element!;
    });
    expect(status.textContent).toMatch(/^stopped/);

    // At the ready state nothing has run yet; at the last step the failed
    // task is an error, the first is executed and the third still pending.
    fireEvent.click(screen.getByTitle('Last step (End)'));
    await waitFor(() => {
      expect(container.querySelector('.df-tree-node-error')).not.toBeNull();
    });
    const nodes = Array.from(container.querySelectorAll('.df-tree-node'));
    const stateOf = (label: string) =>
      nodes
        .find((n) => n.querySelector(':scope > .df-tree-node-content .df-tree-label')?.textContent === label)
        ?.className ?? '';
    expect(stateOf('Write')).toContain('df-tree-node-executed');
    expect(stateOf('Append')).toContain('df-tree-node-error');
    expect(stateOf('Never')).toContain('df-tree-node-pending');
  });

  it('reports a refused definition through the toolbar with its issues', async () => {
    const refused: Workflow[] = [
      { id: 'w', name: 'Refused', tasks: [{ id: 't', name: 'T', function: { name: 'nope', input: {} } }] },
    ];
    const errors: string[] = [];
    const { container } = render(
      <WorkflowVisualizer
        workflows={refused}
        debugConfig={{
          enabled: true,
          engineFactory: defaultEngineFactory,
          initialPayload: {},
          autoExecute: true,
          onExecutionError: (error) => errors.push(`${error.code} ${error.issues?.[0]?.code ?? ''}`),
        }}
      />,
    );
    await waitFor(() => expect(errors).toHaveLength(1));
    expect(errors[0]).toBe('REJECTED UNKNOWN_FUNCTION');
    expect(container.textContent).toContain('UNKNOWN_FUNCTION');
  });
});
