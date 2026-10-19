import { act, render, screen } from '@testing-library/react';
import { createElement } from 'react';
import { expect, it, vi } from 'vitest';
import { LazyLogicEditor } from './LazyLogicEditor';

const deferred = vi.hoisted(() => {
  let resolve!: () => void;
  const ready = new Promise<void>((done) => { resolve = done; });
  return { ready, resolve, received: vi.fn() };
});

vi.mock('@goplasmatic/datalogic-ui', async () => {
  await deferred.ready;
  return { DataLogicEditor: (props: unknown) => {
    deferred.received(props);
    return createElement('div', null, 'Logic editor ready');
  } };
});

it('shows a loading state until the editor loads, then forwards its value and context', async () => {
  const props = { value: { var: 'data.x' }, data: { data: { x: 7 } }, theme: 'dark' as const };
  expect(deferred.received).not.toHaveBeenCalled();
  render(<LazyLogicEditor {...props} />);
  expect(screen.getByRole('status').textContent).toBe('Loading logic editor…');
  await act(async () => deferred.resolve());
  expect(await screen.findByText('Logic editor ready')).toBeDefined();
  expect(deferred.received).toHaveBeenCalledWith(props);
  expect(screen.queryByRole('status')).toBeNull();
});
