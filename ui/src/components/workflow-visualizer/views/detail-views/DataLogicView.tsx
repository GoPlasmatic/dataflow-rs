import { useMemo } from 'react';
import type {
  DataLogicCustomOperator,
  DataLogicEvaluationConfig,
} from '@goplasmatic/datalogic-ui';
import { LazyLogicEditor as DataLogicEditor } from '../../../common/LazyLogicEditor';
import type { JsonLogicValue } from '../../../../types';
import { useTheme } from '../../context';

/**
 * The template-key escape prefix `dataflow-rs` configures on its
 * `datalogic-rs` engine (`TEMPLATE_KEY_ESCAPE` in `src/expr/engine.rs`).
 */
export const DEFAULT_TEMPLATE_KEY_ESCAPE = '$';

/**
 * Look up a dotted path inside a plain object, matching `Secrets::get` in
 * `src/expr/secrets.rs`. An empty path or a missing/non-object segment returns
 * `undefined`.
 */
function lookupSecretPath(
  root: Record<string, unknown> | undefined,
  path: string,
): unknown {
  if (!root || path.length === 0) return undefined;
  const segments = path.split('.');
  let current: unknown = root;
  for (const segment of segments) {
    if (
      segment.length === 0 ||
      current === null ||
      typeof current !== 'object' ||
      Array.isArray(current) ||
      !Object.prototype.hasOwnProperty.call(current, segment)
    ) {
      return undefined;
    }
    current = (current as Record<string, unknown>)[segment];
  }
  return current;
}

/**
 * Creates a `DataLogicCustomOperator` implementing `dataflow-rs`'s reserved
 * `{"secret": "name"}` operator (`SecretOperator` in `src/expr/secrets.rs`).
 */
export function createSecretOperator(
  secrets?: Record<string, unknown>,
): DataLogicCustomOperator {
  return (args: unknown[]) => {
    if (args.length !== 1) {
      throw new Error(`secret: takes exactly one argument, got ${args.length}`);
    }
    const key = args[0];
    if (typeof key !== 'string') {
      throw new Error('secret: the key must be a string');
    }
    if (key.length === 0) {
      throw new Error('secret: the key must not be empty');
    }
    const resolved = lookupSecretPath(secrets, key);
    if (resolved === undefined) {
      throw new Error(`secret '${key}' is not declared`);
    }
    return resolved;
  };
}

export interface DataLogicViewProps {
  value: JsonLogicValue;
  data?: Record<string, unknown>;
  /** Compile multi-key objects and arrays as templates (default: `true`, matching `dataflow-rs`). */
  templating?: boolean;
  /** Escape prefix for literal template keys matching operator names (default: `'$'`, matching `dataflow-rs`). */
  templateKeyEscape?: string;
  /** Optional evaluation configuration forwarded to the datalogic engine. */
  config?: DataLogicEvaluationConfig;
  /** Optional custom operators merged with the built-in `secret` operator. */
  customOperators?: Record<string, DataLogicCustomOperator>;
  /** Optional secrets store for evaluating `{"secret": "name"}` in the expression debugger. */
  secrets?: Record<string, unknown>;
  /** Enable visual editing mode on the underlying `DataLogicEditor`. */
  editable?: boolean;
  /** Callback invoked when the expression is edited in `editable` mode. */
  onChange?: (expression: JsonLogicValue | null) => void;
}

export function DataLogicView({
  value,
  data,
  templating = true,
  templateKeyEscape = DEFAULT_TEMPLATE_KEY_ESCAPE,
  config,
  customOperators,
  secrets,
  editable,
  onChange,
}: DataLogicViewProps) {
  const { resolvedTheme } = useTheme();

  const mergedCustomOperators = useMemo<Record<string, DataLogicCustomOperator>>(
    () => ({
      secret: createSecretOperator(secrets),
      ...customOperators,
    }),
    [secrets, customOperators],
  );

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor" data-theme={resolvedTheme}>
        <DataLogicEditor
          value={value}
          theme={resolvedTheme}
          templating={templating}
          templateKeyEscape={templateKeyEscape}
          config={config}
          customOperators={mergedCustomOperators}
          editable={editable}
          onChange={onChange}
          className="df-datalogic-viewer"
          data={data}
        />
      </div>
    </div>
  );
}
