import { useState, useCallback, useEffect, useRef, useMemo } from 'react';
import { Sun, Moon, BookOpen, ChevronDown, PanelLeftClose, PanelLeft, Braces } from 'lucide-react';

const GithubIcon = ({ size = 16 }: { size?: number }) => (
  <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor">
    <path d="M12 0C5.37 0 0 5.37 0 12c0 5.31 3.435 9.795 8.205 11.385.6.105.825-.255.825-.57 0-.285-.015-1.23-.015-2.235-3.015.555-3.795-.735-4.035-1.41-.135-.345-.72-1.41-1.23-1.695-.42-.225-1.02-.78-.015-.795.945-.015 1.62.87 1.845 1.23 1.08 1.815 2.805 1.305 3.495.99.105-.78.42-1.305.765-1.605-2.67-.3-5.46-1.335-5.46-5.925 0-1.305.465-2.385 1.23-3.225-.12-.3-.54-1.53.12-3.18 0 0 1.005-.315 3.3 1.23.96-.27 1.98-.405 3-.405s2.04.135 3 .405c2.295-1.56 3.3-1.23 3.3-1.23.66 1.65.24 2.88.12 3.18.765.84 1.23 1.905 1.23 3.225 0 4.605-2.805 5.625-5.475 5.925.435.375.81 1.095.81 2.22 0 1.605-.015 2.895-.015 3.3 0 .315.225.69.825.57A12.02 12.02 0 0024 12c0-6.63-5.37-12-12-12z"/>
  </svg>
);

import { WorkflowVisualizer } from './components/workflow-visualizer';
import { JsonEditor, StatusBar } from './components/common';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';
import { defaultEngineFactory } from './engines';
import { useResizable } from './components/workflow-visualizer/hooks';
import { LAYOUT } from './components/workflow-visualizer/constants';
import { ThemeProvider, useTheme } from './components/workflow-visualizer/context';
import initWasm from '@goplasmatic/dataflow-wasm';
import type { Workflow, EngineFactory } from './types';
import { SAMPLE_WORKFLOWS } from './data/sampleWorkflows';
import './App.css';

function AppContent({ engineFactory }: { engineFactory: EngineFactory | undefined }) {
  const { resolvedTheme, setTheme } = useTheme();

  const [workflowsText, setWorkflowsText] = useState('');
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  const [workflowsError, setWorkflowsError] = useState<string | null>(null);

  const [payloadText, setPayloadText] = useState('{}');
  const [payloadError, setPayloadError] = useState<string | null>(null);

  const [selectedExample, setSelectedExample] = useState(Object.keys(SAMPLE_WORKFLOWS)[0]);
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  const [isPanelCollapsed, setIsPanelCollapsed] = useState(false);
  const containerRef = useRef<HTMLElement>(null);

  const [cursorPosition, setCursorPosition] = useState({ line: 1, column: 1 });

  const {
    size: panelWidth,
    isDragging,
    onMouseDown: handleMouseDown,
  } = useResizable({
    containerRef,
    direction: 'horizontal',
    min: LAYOUT.APP_PANEL.MIN,
    max: LAYOUT.APP_PANEL.MAX,
    initial: LAYOUT.APP_PANEL.DEFAULT,
  });

  // Set data-theme on document for App-level styling
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', resolvedTheme);
  }, [resolvedTheme]);

  const toggleTheme = useCallback(() => {
    setTheme(resolvedTheme === 'light' ? 'dark' : 'light');
  }, [resolvedTheme, setTheme]);

  // Handle workflows text change
  const handleWorkflowsChange = useCallback((text: string) => {
    setWorkflowsText(text);
    if (!text.trim()) {
      setWorkflows([]);
      setWorkflowsError(null);
      return;
    }
    try {
      const parsed = JSON.parse(text);
      if (!Array.isArray(parsed)) {
        setWorkflowsError('Workflows must be an array');
        return;
      }
      setWorkflows(parsed);
      setWorkflowsError(null);
    } catch (err) {
      setWorkflowsError(err instanceof Error ? err.message : 'Invalid JSON');
    }
  }, []);

  // Handle payload text change
  const handlePayloadChange = useCallback((text: string) => {
    setPayloadText(text);
    if (!text.trim()) {
      setPayloadError(null);
      return;
    }
    try {
      const parsed = JSON.parse(text);
      if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
        setPayloadError('Payload must be a JSON object');
        return;
      }
      setPayloadError(null);
    } catch (err) {
      setPayloadError(err instanceof Error ? err.message : 'Invalid JSON');
    }
  }, []);

  // Load sample
  const loadSample = useCallback((name: string) => {
    const sample = SAMPLE_WORKFLOWS[name];
    if (sample) {
      setSelectedExample(name);
      setWorkflows(sample.workflows);
      setWorkflowsText(JSON.stringify(sample.workflows, null, 2));
      setWorkflowsError(null);
      setPayloadText(JSON.stringify(sample.payload, null, 2));
      setPayloadError(null);
      setDropdownOpen(false);
    }
  }, []);

  // Load first sample on mount
  useEffect(() => {
    loadSample(Object.keys(SAMPLE_WORKFLOWS)[0]);
  }, [loadSample]);

  // Close dropdown on outside click
  useEffect(() => {
    const handleClickOutside = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setDropdownOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  // Toggle panel
  const togglePanel = useCallback(() => {
    setIsPanelCollapsed((prev) => !prev);
  }, []);

  // Format JSON
  const formatJson = useCallback(() => {
    try {
      const parsedWorkflows = JSON.parse(workflowsText);
      setWorkflowsText(JSON.stringify(parsedWorkflows, null, 2));
    } catch {
      // Ignore formatting errors
    }
    try {
      const parsedPayload = JSON.parse(payloadText);
      setPayloadText(JSON.stringify(parsedPayload, null, 2));
    } catch {
      // Ignore formatting errors
    }
  }, [workflowsText, payloadText]);

  // Keyboard shortcuts
  useKeyboardShortcuts({
    onTogglePanel: togglePanel,
    onFormatJson: formatJson,
  });

  // Handle cursor position change
  const handleCursorChange = useCallback((line: number, column: number) => {
    setCursorPosition({ line, column });
  }, []);

  return (
    <>
      <div className="app" data-theme={resolvedTheme}>
        <header className="app-header">
          <div className="header-title">
            <h1>Dataflow Visualizer</h1>
            <span className="header-subtitle">Workflow Engine Debugger</span>
          </div>
          <div className="header-controls">
            <button
              className="header-action"
              onClick={togglePanel}
              title={isPanelCollapsed ? 'Show Editor (Ctrl+B)' : 'Hide Editor (Ctrl+B)'}
            >
              {isPanelCollapsed ? <PanelLeft size={16} /> : <PanelLeftClose size={16} />}
            </button>
            <button
              className="header-action"
              onClick={formatJson}
              title="Format JSON (Ctrl+Shift+F)"
            >
              <Braces size={16} />
              <span>Format</span>
            </button>
            <div className="header-divider" />
            <a
              href="https://github.com/GoPlasmatic/dataflow-rs"
              target="_blank"
              rel="noopener noreferrer"
              className="header-link"
            >
              <GithubIcon size={16} />
              <span>GitHub</span>
            </a>
            <a
              href="https://goplasmatic.github.io/dataflow-rs/"
              target="_blank"
              rel="noopener noreferrer"
              className="header-link"
            >
              <BookOpen size={16} />
              <span>Docs</span>
            </a>
            <div className="header-divider" />
            <div className="examples-dropdown" ref={dropdownRef}>
              <button
                className="examples-dropdown-trigger"
                onClick={() => setDropdownOpen(!dropdownOpen)}
              >
                <span className="examples-dropdown-label">Examples</span>
                <span className="examples-dropdown-value">{selectedExample}</span>
                <ChevronDown size={14} className={`dropdown-icon ${dropdownOpen ? 'open' : ''}`} />
              </button>
              {dropdownOpen && (
                <div className="examples-dropdown-menu">
                  {Object.keys(SAMPLE_WORKFLOWS).map((name) => (
                    <button
                      key={name}
                      className={`examples-dropdown-item ${name === selectedExample ? 'selected' : ''}`}
                      onClick={() => loadSample(name)}
                    >
                      {name}
                    </button>
                  ))}
                </div>
              )}
            </div>
            <button className="theme-toggle" onClick={toggleTheme} title="Toggle Theme">
              {resolvedTheme === 'light' ? <Moon size={18} /> : <Sun size={18} />}
            </button>
          </div>
        </header>

        <main className="app-main" ref={containerRef}>
          {/* Left Panel - JSON Editors */}
          <div
            className={`panel editor-panel ${isPanelCollapsed ? 'collapsed' : ''}`}
            style={{ width: isPanelCollapsed ? 0 : panelWidth }}
          >
            <div className="editor-section">
              <div className="editor-header">
                <h3>Workflows</h3>
                {workflowsError && <span className="editor-error">{workflowsError}</span>}
              </div>
              <div className="editor-content">
                <JsonEditor
                  value={workflowsText}
                  onChange={handleWorkflowsChange}
                  theme={resolvedTheme}
                  onCursorChange={handleCursorChange}
                />
              </div>
            </div>
            <div className="editor-section">
              <div className="editor-header">
                <h3>Payload</h3>
                {payloadError && <span className="editor-error">{payloadError}</span>}
              </div>
              <div className="editor-content">
                <JsonEditor
                  value={payloadText}
                  onChange={handlePayloadChange}
                  theme={resolvedTheme}
                  onCursorChange={handleCursorChange}
                />
              </div>
            </div>
          </div>

          {/* Divider */}
          {!isPanelCollapsed && (
            <div
              className={`divider ${isDragging ? 'dragging' : ''}`}
              onMouseDown={handleMouseDown}
            />
          )}

          {/* Right Panel - Visualizer */}
          <div className="panel visual-panel">
            <div className="panel-content">
              <WorkflowVisualizer
                key={workflowsText}
                workflows={workflows}
                theme={resolvedTheme}
                debugConfig={{
                  enabled: true,
                  engineFactory: engineFactory!,
                  autoExecute: true,
                }}
                debugPayload={(() => {
                  try {
                    return JSON.parse(payloadText);
                  } catch {
                    return {};
                  }
                })()}
              />
            </div>
          </div>
        </main>

        {/* Status Bar */}
        <StatusBar
          workflows={workflows}
          workflowsError={workflowsError}
          messageError={payloadError}
          cursorPosition={cursorPosition}
        />
      </div>
    </>
  );
}

function App() {
  const [wasmReady, setWasmReady] = useState(false);

  // Initialize WASM module
  useEffect(() => {
    initWasm()
      .then(() => {
        setWasmReady(true);
        console.log('WASM module initialized');
      })
      .catch((err) => {
        console.error('Failed to initialize WASM:', err);
      });
  }, []);

  // Only provide engineFactory once WASM is ready
  const engineFactory = useMemo(
    () => (wasmReady ? defaultEngineFactory : undefined),
    [wasmReady]
  );

  return (
    <ThemeProvider defaultTheme="system">
      <AppContent engineFactory={engineFactory} />
    </ThemeProvider>
  );
}

export default App;
