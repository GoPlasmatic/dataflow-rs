import { DataLogicEditor } from '@goplasmatic/datalogic-ui';
import type { JsonLogicValue } from '../../../../types';
import { useTheme } from '../../context';

interface DataLogicViewProps {
  value: JsonLogicValue;
}

export function DataLogicView({ value }: DataLogicViewProps) {
  const { resolvedTheme } = useTheme();

  return (
    <div className="df-details-content">
      <div className="df-details-logic-editor" data-theme={resolvedTheme}>
        <DataLogicEditor
          value={value}
          theme={resolvedTheme}
          preserveStructure={true}
          className="df-datalogic-viewer"
        />
      </div>
    </div>
  );
}
