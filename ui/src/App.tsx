import { useState, useCallback, useEffect, useRef } from 'react';
import { Sun, Moon, Github, BookOpen, ChevronDown, PanelLeftClose, PanelLeft, Braces, Play, CheckCircle, XCircle, ChevronLeft, ChevronRight, Pause, Square } from 'lucide-react';
import { WorkflowVisualizer, DebuggerProvider, useDebugger } from './components/workflow-visualizer';
import { JsonEditor, StatusBar } from './components/common';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';
import initWasm, { WasmEngine } from '@goplasmatic/dataflow-wasm';
import type { Workflow, ExecutionTrace } from './types';
import { getMessageAtStep } from './types';
import './App.css';

// Sample workflows for demonstration (using built-in functions: parse_json, map, validation)
// All workflows follow the recommended pattern: parse_json first to load payload into data context
const SAMPLE_WORKFLOWS: Record<string, { workflows: Workflow[]; payload: object }> = {
  'User Processing': {
    workflows: [
      {
        id: 'user-processing',
        name: 'User Processing',
        priority: 0,
        tasks: [
          {
            id: 'load-payload',
            name: 'Load Payload',
            function: {
              name: 'parse_json',
              input: {
                source: 'payload',
                target: 'input',
              },
            },
          },
          {
            id: 'init-user',
            name: 'Initialize User',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.user.id', logic: { var: 'data.input.id' } },
                  { path: 'data.user.full_name', logic: { cat: [{ var: 'data.input.first_name' }, ' ', { var: 'data.input.last_name' }] } },
                  { path: 'data.user.email', logic: { var: 'data.input.email' } },
                ],
              },
            },
          },
          {
            id: 'validate-user',
            name: 'Validate User',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '!!': { var: 'data.user.full_name' } }, message: 'Name required' },
                  { logic: { '!!': { var: 'data.user.email' } }, message: 'Email required' },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: { id: '123', first_name: 'John', last_name: 'Doe', email: 'john@example.com' },
  },
  'Order Processing': {
    workflows: [
      {
        id: 'order-processing',
        name: 'Order Processing',
        priority: 0,
        tasks: [
          {
            id: 'load-payload',
            name: 'Load Payload',
            function: {
              name: 'parse_json',
              input: {
                source: 'payload',
                target: 'input',
              },
            },
          },
          {
            id: 'parse-order',
            name: 'Parse Order',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.order.id', logic: { var: 'data.input.order_id' } },
                  { path: 'data.order.total', logic: { var: 'data.input.amount' } },
                ],
              },
            },
          },
          {
            id: 'calculate-tax',
            name: 'Calculate Tax',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.order.tax', logic: { '*': [{ var: 'data.order.total' }, 0.1] } },
                  { path: 'data.order.grand_total', logic: { '+': [{ var: 'data.order.total' }, { var: 'data.order.tax' }] } },
                ],
              },
            },
          },
          {
            id: 'validate-order',
            name: 'Validate Order',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '>': [{ var: 'data.order.total' }, 0] }, message: 'Order total must be positive' },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: { order_id: 'ORD-001', amount: 150.00 },
  },
  'Simple Validation': {
    workflows: [
      {
        id: 'validate-input',
        name: 'Input Validation',
        priority: 0,
        tasks: [
          {
            id: 'load-payload',
            name: 'Load Payload',
            function: {
              name: 'parse_json',
              input: {
                source: 'payload',
                target: 'input',
              },
            },
          },
          {
            id: 'check-required',
            name: 'Check Required Fields',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '!!': { var: 'data.input.name' } }, message: 'Name is required' },
                  { logic: { '!!': { var: 'data.input.email' } }, message: 'Email is required' },
                ],
              },
            },
          },
          {
            id: 'transform',
            name: 'Transform Data',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.formatted_name', logic: { upper: { var: 'data.input.name' } } },
                  { path: 'data.email_lower', logic: { lower: { var: 'data.input.email' } } },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: { name: 'alice', email: 'Alice@Test.com' },
  },
  'Data Pipeline': {
    workflows: [
      {
        id: 'input-mapping',
        name: 'Input Mapping',
        priority: 0,
        tasks: [
          {
            id: 'load-payload',
            name: 'Load Payload',
            function: {
              name: 'parse_json',
              input: {
                source: 'payload',
                target: 'input',
              },
            },
          },
          {
            id: 'extract-fields',
            name: 'Extract Fields',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.customer.id', logic: { var: 'data.input.customer_id' } },
                  { path: 'data.customer.name', logic: { var: 'data.input.customer_name' } },
                  { path: 'data.items', logic: { var: 'data.input.line_items' } },
                  { path: 'data.pricing.subtotal', logic: { var: 'data.input.subtotal' } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'input-validation',
        name: 'Input Validation',
        priority: 1,
        tasks: [
          {
            id: 'validate-customer',
            name: 'Validate Customer',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '!!': { var: 'data.customer.id' } }, message: 'Customer ID required' },
                  { logic: { '!!': { var: 'data.customer.name' } }, message: 'Customer name required' },
                ],
              },
            },
          },
          {
            id: 'validate-items',
            name: 'Validate Items',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '!!': { 'var': 'data.items.0' } }, message: 'At least one item required' },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'pricing-calc',
        name: 'Pricing Calculation',
        priority: 2,
        tasks: [
          {
            id: 'calc-tax',
            name: 'Calculate Tax',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.pricing.tax_rate', logic: 0.08 },
                  { path: 'data.pricing.tax', logic: { '*': [{ var: 'data.pricing.subtotal' }, 0.08] } },
                ],
              },
            },
          },
          {
            id: 'calc-total',
            name: 'Calculate Total',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.pricing.total', logic: { '+': [{ var: 'data.pricing.subtotal' }, { var: 'data.pricing.tax' }] } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'discount-check',
        name: 'Discount Processing',
        priority: 3,
        tasks: [
          {
            id: 'apply-discount',
            name: 'Apply Discount',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.pricing.discount', logic: { '*': [{ var: 'data.pricing.subtotal' }, 0.1] } },
                  { path: 'data.pricing.total', logic: { '-': [{ var: 'data.pricing.total' }, { var: 'data.pricing.discount' }] } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'shipping-calc',
        name: 'Shipping Calculation',
        priority: 4,
        tasks: [
          {
            id: 'determine-shipping',
            name: 'Determine Shipping',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.shipping.method', logic: { if: [{ '>': [{ var: 'data.pricing.subtotal' }, 100] }, 'free', 'standard'] } },
                  { path: 'data.shipping.cost', logic: { if: [{ '>': [{ var: 'data.pricing.subtotal' }, 100] }, 0, 9.99] } },
                ],
              },
            },
          },
          {
            id: 'add-shipping',
            name: 'Add Shipping to Total',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.pricing.grand_total', logic: { '+': [{ var: 'data.pricing.total' }, { var: 'data.shipping.cost' }] } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'order-validation',
        name: 'Order Validation',
        priority: 5,
        tasks: [
          {
            id: 'validate-totals',
            name: 'Validate Totals',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '>': [{ var: 'data.pricing.grand_total' }, 0] }, message: 'Grand total must be positive' },
                  { logic: { '>=': [{ var: 'data.pricing.subtotal' }, { var: 'data.pricing.discount' }] }, message: 'Discount cannot exceed subtotal' },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'output-mapping',
        name: 'Output Mapping',
        priority: 6,
        tasks: [
          {
            id: 'format-response',
            name: 'Format Response',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.response.order_id', logic: { cat: ['ORD-', { var: 'data.customer.id' }, '-', { substr: [{ var: 'temp_data.timestamp' }, 0, 8] }] } },
                  { path: 'data.response.status', logic: 'confirmed' },
                  { path: 'data.response.customer_name', logic: { var: 'data.customer.name' } },
                  { path: 'data.response.total', logic: { var: 'data.pricing.grand_total' } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'audit-trail',
        name: 'Audit Trail',
        priority: 7,
        continue_on_error: true,
        tasks: [
          {
            id: 'create-audit',
            name: 'Create Audit Record',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.audit.processed', logic: true },
                  { path: 'data.audit.customer_id', logic: { var: 'data.customer.id' } },
                  { path: 'data.audit.total_amount', logic: { var: 'data.pricing.grand_total' } },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: {
      customer_id: 'CUST-001',
      customer_name: 'Acme Corp',
      line_items: [
        { sku: 'ITEM-A', qty: 2, price: 25.00 },
        { sku: 'ITEM-B', qty: 1, price: 50.00 },
      ],
      subtotal: 100.00,
    },
  },
};

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

// Debug controls component that uses the debugger context
function DebugControls({
  workflows,
  payloadText,
  payloadError,
  wasmReady,
}: {
  workflows: Workflow[];
  payloadText: string;
  payloadError: string | null;
  wasmReady: boolean;
}) {
  const {
    state,
    hasTrace,
    isAtStart,
    isAtEnd,
    totalSteps,
    executeTrace,
    reset,
    stepForward,
    stepBackward,
    play,
    pause,
  } = useDebugger();

  const [executionSuccess, setExecutionSuccess] = useState<boolean | null>(null);
  const lastExecutionRef = useRef<{ workflows: string; payload: string } | null>(null);

  const runDebug = useCallback(async () => {
    if (!wasmReady || workflows.length === 0 || payloadError) return;

    // Check if this is the same execution
    const workflowsJson = JSON.stringify(workflows);
    const current = { workflows: workflowsJson, payload: payloadText };
    if (lastExecutionRef.current?.workflows === current.workflows &&
        lastExecutionRef.current?.payload === current.payload) {
      return; // Skip if same as last execution
    }

    setExecutionSuccess(null);
    reset();

    try {
      const payload = JSON.parse(payloadText);
      const engine = new WasmEngine(workflowsJson);

      // Use process_with_trace for step-by-step debugging
      const traceJson = await engine.process_with_trace(JSON.stringify(payload));
      const trace: ExecutionTrace = JSON.parse(traceJson);

      executeTrace(trace);
      lastExecutionRef.current = current;

      // Check if execution was successful
      const finalMessage = trace.steps.length > 0
        ? getMessageAtStep(trace, trace.steps.length - 1)
        : null;
      setExecutionSuccess(finalMessage ? finalMessage.errors.length === 0 : true);

      engine.free();
    } catch (err) {
      console.error('Execution error:', err);
      setExecutionSuccess(false);
    }
  }, [wasmReady, workflows, payloadText, payloadError, executeTrace, reset]);

  // Auto-run when workflows or payload change
  useEffect(() => {
    if (!wasmReady || workflows.length === 0 || payloadError) return;

    // Debounce the auto-run
    const timeoutId = setTimeout(() => {
      runDebug();
    }, 500);

    return () => clearTimeout(timeoutId);
  }, [wasmReady, workflows, payloadText, payloadError, runDebug]);

  const handleReset = useCallback(() => {
    reset();
    setExecutionSuccess(null);
    // Clear the ref so auto-run will re-execute
    lastExecutionRef.current = null;
  }, [reset]);

  return (
    <div className="debug-controls-inline">
      {executionSuccess !== null && (
        <span className={`execution-status ${executionSuccess ? 'success' : 'error'}`}>
          {executionSuccess ? <CheckCircle size={14} /> : <XCircle size={14} />}
          {executionSuccess ? 'Success' : 'Error'}
        </span>
      )}

      {/* Always show step counter */}
      <span className="step-counter">
        {hasTrace
          ? (state.currentStepIndex >= 0
              ? `Step ${state.currentStepIndex + 1} / ${totalSteps}`
              : `Ready (${totalSteps} steps)`)
          : 'Ready'}
      </span>

      {/* Always show step controls */}
      <div className="step-controls">
        <button
          className="step-btn"
          onClick={stepBackward}
          disabled={!hasTrace || isAtStart}
          title="Previous Step"
        >
          <ChevronLeft size={16} />
        </button>

        {state.playbackState === 'playing' ? (
          <button
            className="step-btn"
            onClick={pause}
            title="Pause"
          >
            <Pause size={14} />
          </button>
        ) : (
          <button
            className="step-btn"
            onClick={play}
            disabled={!hasTrace || isAtEnd}
            title="Play"
          >
            <Play size={14} />
          </button>
        )}

        <button
          className="step-btn"
          onClick={stepForward}
          disabled={!hasTrace || isAtEnd}
          title="Next Step"
        >
          <ChevronRight size={16} />
        </button>

        <button
          className="step-btn"
          onClick={handleReset}
          disabled={!hasTrace}
          title="Stop"
        >
          <Square size={12} />
        </button>
      </div>

    </div>
  );
}

function AppContent() {
  const { theme, toggleTheme } = useTheme();

  const [workflowsText, setWorkflowsText] = useState('');
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  const [workflowsError, setWorkflowsError] = useState<string | null>(null);

  const [payloadText, setPayloadText] = useState('{}');
  const [payloadError, setPayloadError] = useState<string | null>(null);

  const [selectedExample, setSelectedExample] = useState(Object.keys(SAMPLE_WORKFLOWS)[0]);
  const [dropdownOpen, setDropdownOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  // WASM initialization state
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
              wasmReady={wasmReady}
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
  );
}

function App() {
  return (
    <DebuggerProvider autoActivate={true}>
      <AppContent />
    </DebuggerProvider>
  );
}

export default App;
