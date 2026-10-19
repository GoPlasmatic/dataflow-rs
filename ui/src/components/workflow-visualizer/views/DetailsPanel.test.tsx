import { render, waitFor } from '@testing-library/react';
import { expect, it } from 'vitest';
import { DetailsPanel } from './DetailsPanel';
import { ThemeProvider } from '../context';
import type { Workflow } from '../../../types';

it('loads workflow and folder diagrams when selected', async () => {
  const workflow: Workflow = {
    id: 'w', name: 'Workflow',
    tasks: [{ id: 't', name: 'Task', function: { name: 'log', input: { message: 'hello' } } }],
  };
  const onSelect = () => {};
  const { container, rerender } = render(
    <ThemeProvider><DetailsPanel selection={{ type: 'none' }} onSelect={onSelect} /></ThemeProvider>
  );
  expect(container.querySelector('.react-flow')).toBeNull();
  rerender(<ThemeProvider><DetailsPanel selection={{ type: 'workflow', workflow }} onSelect={onSelect} /></ThemeProvider>);
  await waitFor(() => expect(container.querySelector('.react-flow')).not.toBeNull());
  rerender(<ThemeProvider><DetailsPanel selection={{ type: 'folder', workflows: [workflow], name: 'Folder' }} onSelect={onSelect} /></ThemeProvider>);
  await waitFor(() => expect(container.querySelector('.react-flow')).not.toBeNull());
});
