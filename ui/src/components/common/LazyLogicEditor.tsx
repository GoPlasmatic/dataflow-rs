import { lazy, Suspense } from 'react';
import type { DataLogicEditorProps, OPERATORS } from '@goplasmatic/datalogic-ui';

type OperatorConfig = (typeof OPERATORS)[string];

/**
 * Operator metadata for `dataflow-rs`'s reserved `{"secret": "name"}` operator,
 * registered into `@goplasmatic/datalogic-ui`'s operator table so static and
 * trace views render `{"secret": ...}` as an operator node rather than a
 * literal object template.
 */
const SECRET_OPERATOR_CONFIG: OperatorConfig = {
  name: 'secret',
  label: 'Secret',
  category: 'variable',
  description: 'Read a value from the engine secrets store by dotted key',
  arity: {
    type: 'unary',
    min: 1,
    max: 1,
    args: [
      {
        name: 'key',
        label: 'Secret Key',
        type: 'string',
        required: true,
        description: 'Dotted path into the engine secrets store',
      },
    ],
  },
  help: {
    summary: 'Read a secret from the engine secrets store',
    details:
      'Evaluates to the value stored under the given dotted key in the engine secrets store. Secret values are never recorded in traces.',
    returnType: 'any',
    examples: [
      {
        title: 'Read a secret',
        rule: { secret: 'partner.hmac' },
        result: '******',
      },
    ],
  },
  ui: {
    icon: 'database',
    shortLabel: 'secret',
    nodeType: 'operator',
  },
};

// The explorer and run results do not need the logic editor or its WASM.
const Editor = lazy(() => import('@goplasmatic/datalogic-ui')
  .then((mod) => {
    if (mod.OPERATORS && !mod.OPERATORS.secret) {
      mod.OPERATORS.secret = SECRET_OPERATOR_CONFIG;
    }
    return { default: mod.DataLogicEditor };
  }));

export function LazyLogicEditor(props: DataLogicEditorProps) {
  return (
    <Suspense fallback={<div className="df-details-loading" role="status">Loading logic editor…</div>}>
      <Editor {...props} />
    </Suspense>
  );
}
