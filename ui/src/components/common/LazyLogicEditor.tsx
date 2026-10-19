import { lazy, Suspense } from 'react';
import type { DataLogicEditorProps } from '@goplasmatic/datalogic-ui';

// The explorer and run results do not need the logic editor or its WASM.
const Editor = lazy(() => import('@goplasmatic/datalogic-ui')
  .then(({ DataLogicEditor }) => ({ default: DataLogicEditor })));

export function LazyLogicEditor(props: DataLogicEditorProps) {
  return (
    <Suspense fallback={<div className="df-details-loading" role="status">Loading logic editor…</div>}>
      <Editor {...props} />
    </Suspense>
  );
}
