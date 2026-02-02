import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { JsonLogicValue } from '../../../../types';
import { useTheme } from '../../context';

interface DataLogicViewProps {
  value: JsonLogicValue;
  data?: Record<string, unknown>;
}

export function DataLogicView({ value, data }: DataLogicViewProps) {
  const { resolvedTheme } = useTheme();

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor" data-theme={resolvedTheme}>
        <DataLogicEditor
          value={value}
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
          data={data}
        />
      </div>
    </div>
  );
}
