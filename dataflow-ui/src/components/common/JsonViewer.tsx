import { useState } from 'react';
import { ChevronRight, ChevronDown } from 'lucide-react';

interface JsonViewerProps {
  data: unknown;
  initialExpanded?: boolean;
  maxStringLength?: number;
  className?: string;
}

export function JsonViewer({
  data,
  initialExpanded = false,
  maxStringLength = 50,
  className = '',
}: JsonViewerProps) {
  return (
    <div className={`df-json-viewer ${className}`}>
      <JsonNode
        data={data}
        initialExpanded={initialExpanded}
        maxStringLength={maxStringLength}
      />
    </div>
  );
}

interface JsonNodeProps {
  data: unknown;
  initialExpanded: boolean;
  maxStringLength: number;
  keyName?: string;
  isLast?: boolean;
}

function JsonNode({
  data,
  initialExpanded,
  maxStringLength,
  keyName,
  isLast = true,
}: JsonNodeProps) {
  const [expanded, setExpanded] = useState(initialExpanded);

  const renderValue = () => {
    if (data === null) {
      return <span className="df-json-null">null</span>;
    }

    if (data === undefined) {
      return <span className="df-json-undefined">undefined</span>;
    }

    if (typeof data === 'boolean') {
      return <span className="df-json-boolean">{data.toString()}</span>;
    }

    if (typeof data === 'number') {
      return <span className="df-json-number">{data}</span>;
    }

    if (typeof data === 'string') {
      const display =
        data.length > maxStringLength
          ? `${data.slice(0, maxStringLength)}...`
          : data;
      return <span className="df-json-string">"{display}"</span>;
    }

    if (Array.isArray(data)) {
      if (data.length === 0) {
        return <span className="df-json-bracket">[]</span>;
      }

      return (
        <span className="df-json-array">
          <button
            onClick={() => setExpanded(!expanded)}
            className="df-json-toggle"
          >
            {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </button>
          <span className="df-json-bracket">[</span>
          {!expanded && (
            <span className="df-json-preview">
              {data.length} item{data.length !== 1 ? 's' : ''}
            </span>
          )}
          {expanded && (
            <div className="df-json-children">
              {/* Index key is acceptable here: array items are immutable display data */}
              {data.map((item, index) => (
                <JsonNode
                  key={index}
                  data={item}
                  initialExpanded={false}
                  maxStringLength={maxStringLength}
                  isLast={index === data.length - 1}
                />
              ))}
            </div>
          )}
          <span className="df-json-bracket">]</span>
        </span>
      );
    }

    if (typeof data === 'object') {
      const entries = Object.entries(data as Record<string, unknown>);
      if (entries.length === 0) {
        return <span className="df-json-bracket">{'{}'}</span>;
      }

      return (
        <span className="df-json-object">
          <button
            onClick={() => setExpanded(!expanded)}
            className="df-json-toggle"
          >
            {expanded ? <ChevronDown size={12} /> : <ChevronRight size={12} />}
          </button>
          <span className="df-json-bracket">{'{'}</span>
          {!expanded && (
            <span className="df-json-preview">
              {entries.length} key{entries.length !== 1 ? 's' : ''}
            </span>
          )}
          {expanded && (
            <div className="df-json-children">
              {entries.map(([key, value], index) => (
                <JsonNode
                  key={key}
                  keyName={key}
                  data={value}
                  initialExpanded={false}
                  maxStringLength={maxStringLength}
                  isLast={index === entries.length - 1}
                />
              ))}
            </div>
          )}
          <span className="df-json-bracket">{'}'}</span>
        </span>
      );
    }

    return <span>{String(data)}</span>;
  };

  return (
    <div className="df-json-node">
      {keyName !== undefined && (
        <>
          <span className="df-json-key">"{keyName}"</span>
          <span className="df-json-colon">: </span>
        </>
      )}
      {renderValue()}
      {!isLast && <span className="df-json-comma">,</span>}
    </div>
  );
}
