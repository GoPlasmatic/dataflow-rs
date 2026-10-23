import { act, cleanup, render, screen } from '@testing-library/react';
import { createElement } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { LazyLogicEditor } from './LazyLogicEditor';
import {
  DataLogicView,
  DEFAULT_TEMPLATE_KEY_ESCAPE,
  createSecretOperator,
} from '../workflow-visualizer/views/detail-views/DataLogicView';
import { ThemeProvider } from '../workflow-visualizer/context';

afterEach(cleanup);

const deferred = vi.hoisted(() => {
  let resolve!: () => void;
  const ready = new Promise<void>((done) => { resolve = done; });
  return { ready, resolve, received: vi.fn(), operators: {} as Record<string, unknown> };
});

vi.mock('@goplasmatic/datalogic-ui', async () => {
  await deferred.ready;
  return {
    OPERATORS: deferred.operators,
    DataLogicEditor: (props: unknown) => {
      deferred.received(props);
      return createElement('div', null, 'Logic editor ready');
    },
  };
});

it('shows a loading state until the editor loads, then forwards its value and context', async () => {
  const props = { value: { var: 'data.x' }, data: { data: { x: 7 } }, theme: 'dark' as const };
  expect(deferred.received).not.toHaveBeenCalled();
  render(<LazyLogicEditor {...props} />);
  expect(screen.getByRole('status').textContent).toBe('Loading logic editor…');
  await act(async () => deferred.resolve());
  expect(await screen.findByText('Logic editor ready')).toBeDefined();
  expect(deferred.received).toHaveBeenCalledWith(props);
  expect(deferred.operators.secret).toBeDefined();
  expect(screen.queryByRole('status')).toBeNull();
});

it('configures DataLogicView with dataflow-rs templateKeyEscape and secret operator', async () => {
  deferred.received.mockClear();
  render(
    <ThemeProvider defaultTheme="dark">
      <DataLogicView
        value={{ '$var': { secret: 'partner.hmac' } }}
        data={{ data: {} }}
        secrets={{ partner: { hmac: 'hmac-value' } }}
      />
    </ThemeProvider>,
  );
  expect(await screen.findByText('Logic editor ready')).toBeDefined();
  expect(deferred.received).toHaveBeenCalledTimes(1);
  const forwarded = deferred.received.mock.calls[0][0] as {
    templating: boolean;
    templateKeyEscape: string;
    customOperators: Record<string, (args: unknown[]) => unknown>;
  };
  expect(forwarded.templating).toBe(true);
  expect(forwarded.templateKeyEscape).toBe(DEFAULT_TEMPLATE_KEY_ESCAPE);
  expect(forwarded.customOperators.secret(['partner.hmac'])).toBe('hmac-value');
});

it('validates secret operator arguments and missing keys', () => {
  const secretOp = createSecretOperator({ api_token: 'tok-123', nested: { key: 42 } });
  expect(secretOp(['api_token'])).toBe('tok-123');
  expect(secretOp(['nested.key'])).toBe(42);
  expect(() => secretOp([])).toThrow('secret: takes exactly one argument, got 0');
  expect(() => secretOp([123])).toThrow('secret: the key must be a string');
  expect(() => secretOp([''])).toThrow('secret: the key must not be empty');
  expect(() => secretOp(['missing'])).toThrow("secret 'missing' is not declared");
});
