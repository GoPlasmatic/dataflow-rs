import { useState, useCallback, useEffect, useRef, useMemo } from 'react';
import { Sun, Moon, Github, BookOpen, ChevronDown, PanelLeftClose, PanelLeft, Braces } from 'lucide-react';
import { WorkflowVisualizer, DebuggerProvider } from './components/workflow-visualizer';
import { JsonEditor, StatusBar } from './components/common';
import { DebugControls } from './components/DebugControls';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';
import { defaultEngineFactory } from './engines';
import initWasm from '@goplasmatic/dataflow-wasm';
import type { Workflow, EngineFactory } from './types';
import { SAMPLE_WORKFLOWS } from './data/sampleWorkflows';
import './App.css';

function useTheme() {
  const [theme, setTheme] = useState<'light' | 'dark'>(() => {
    if (typeof window !== 'undefined') {
      return window.matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
    }
    return 'light';
  });

  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);

  const toggleTheme = useCallback(() => {
    setTheme((t) => (t === 'light' ? 'dark' : 'light'));
  }, []);

  return { theme, toggleTheme };
}

function AppContent({ engineFactory }: { engineFactory: EngineFactory | undefined }) {
  const { theme, toggleTheme } = useTheme();

  const [workflowsText, setWorkflowsText] = useState('');
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  const [workflowsError, setWorkflowsError] = useState<string | null>(null);

  const [payloadText, setPayloadText] = useState('{}');
  const [payloadError, setPayloadError] = useState<string | null>(null);

  const [selectedExample, setSelectedExample] = useState(Object.keys(SAMPLE_WORKFLOWS)[0]);
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  const [panelWidth, setPanelWidth] = useState(400);
  const [isPanelCollapsed, setIsPanelCollapsed] = useState(false);
  const [isDragging, setIsDragging] = useState(false);
  const containerRef = useRef<HTMLElement>(null);

  const [cursorPosition, setCursorPosition] = useState({ line: 1, column: 1 });

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

  // Divider dragging
  const handleMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsDragging(true);
  }, []);

  useEffect(() => {
    if (!isDragging) return;
    const handleMouseMove = (e: MouseEvent) => {
      if (containerRef.current) {
        const rect = containerRef.current.getBoundingClientRect();
        setPanelWidth(Math.max(250, Math.min(600, e.clientX - rect.left)));
      }
    };
    const handleMouseUp = () => setIsDragging(false);
    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);
    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
    };
  }, [isDragging]);

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
    <DebuggerProvider autoActivate={true} engineFactory={engineFactory}>
      <div className="app" data-theme={theme}>
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
              href="https://github.com/AkshitVadodariya1201/dataflow-rs"
              target="_blank"
              rel="noopener noreferrer"
              className="header-link"
            >
              <Github size={16} />
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
              {theme === 'light' ? <Moon size={18} /> : <Sun size={18} />}
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
                  theme={theme}
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
                  theme={theme}
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

          {/* Right Panel - Visual Flow */}
          <div className="panel visual-panel">
            <div className="panel-header">
              <h2>Workflow Flow</h2>
              <DebugControls
                workflows={workflows}
                payloadText={payloadText}
                payloadError={payloadError}
              />
            </div>
            <div className="panel-content">
              <WorkflowVisualizer
                key={workflowsText}
                workflows={workflows}
                theme={theme}
                debugMode={true}
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
    </DebuggerProvider>
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

  return <AppContent engineFactory={engineFactory} />;
}

export default App;
