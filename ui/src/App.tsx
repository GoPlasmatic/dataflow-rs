import { useState, useCallback, useEffect, useRef } from 'react';
import { Sun, Moon, Github, BookOpen, ChevronDown, PanelLeftClose, PanelLeft, Braces } from 'lucide-react';
import { WorkflowVisualizer } from './components/workflow-visualizer';
import { JsonEditor, StatusBar } from './components/common';
import { useKeyboardShortcuts } from './hooks/useKeyboardShortcuts';
import type { Workflow } from './types';
import './App.css';

// Sample workflows for demonstration (using only built-in functions: map, validation)
const SAMPLE_WORKFLOWS: Record<string, { workflows: Workflow[]; message: object }> = {
  'User & Order Processing': {
    workflows: [
      {
        id: 'user-processing',
        name: 'User Processing',
        priority: 0,
        condition: { '==': [{ var: 'metadata.type' }, 'user'] },
        tasks: [
          {
            id: 'init-user',
            name: 'Initialize User',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.user.id', logic: { var: 'payload.id' } },
                  { path: 'data.user.full_name', logic: { cat: [{ var: 'payload.first_name' }, ' ', { var: 'payload.last_name' }] } },
                  { path: 'data.user.email', logic: { var: 'payload.email' } },
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
      {
        id: 'order-processing',
        name: 'Order Processing',
        priority: 1,
        condition: { '==': [{ var: 'metadata.type' }, 'order'] },
        tasks: [
          {
            id: 'parse-order',
            name: 'Parse Order',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.order.id', logic: { var: 'payload.order_id' } },
                  { path: 'data.order.total', logic: { var: 'payload.amount' } },
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
    message: {
      metadata: { type: 'user', notify: true },
      payload: { id: '123', first_name: 'John', last_name: 'Doe', email: 'john@example.com' },
    },
  },
  'Simple Validation': {
    workflows: [
      {
        id: 'validate-input',
        name: 'Input Validation',
        priority: 0,
        tasks: [
          {
            id: 'check-required',
            name: 'Check Required Fields',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '!!': { var: 'payload.name' } }, message: 'Name is required' },
                  { logic: { '!!': { var: 'payload.email' } }, message: 'Email is required' },
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
                  { path: 'data.formatted_name', logic: { upper: { var: 'payload.name' } } },
                  { path: 'data.email_lower', logic: { lower: { var: 'payload.email' } } },
                ],
              },
            },
          },
        ],
      },
    ],
    message: {
      metadata: {},
      payload: { name: 'alice', email: 'Alice@Test.com' },
    },
  },
  'Data Pipeline (8 Workflows)': {
    workflows: [
      {
        id: 'input-mapping',
        name: 'Input Mapping',
        priority: 0,
        tasks: [
          {
            id: 'extract-fields',
            name: 'Extract Fields',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.customer.id', logic: { var: 'payload.customer_id' } },
                  { path: 'data.customer.name', logic: { var: 'payload.customer_name' } },
                  { path: 'data.items', logic: { var: 'payload.line_items' } },
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
                  { logic: { '>': [{ var: 'data.items.length' }, 0] }, message: 'At least one item required' },
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
        condition: { '>': [{ var: 'metadata.item_count' }, 0] },
        tasks: [
          {
            id: 'calc-subtotal',
            name: 'Calculate Subtotal',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.pricing.subtotal', logic: { var: 'payload.subtotal' } },
                ],
              },
            },
          },
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
        condition: { '!!': { var: 'metadata.coupon_code' } },
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
    message: {
      metadata: { item_count: 3, coupon_code: 'SAVE10' },
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

function App() {
  const { theme, toggleTheme } = useTheme();

  const [workflowsText, setWorkflowsText] = useState('');
  const [workflows, setWorkflows] = useState<Workflow[]>([]);
  const [workflowsError, setWorkflowsError] = useState<string | null>(null);

  const [messageText, setMessageText] = useState('{}');
  const [messageError, setMessageError] = useState<string | null>(null);

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

  // Handle message text change
  const handleMessageChange = useCallback((text: string) => {
    setMessageText(text);
    if (!text.trim()) {
      setMessageError(null);
      return;
    }
    try {
      const parsed = JSON.parse(text);
      if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
        setMessageError('Message must be a JSON object');
        return;
      }
      setMessageError(null);
    } catch (err) {
      setMessageError(err instanceof Error ? err.message : 'Invalid JSON');
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
      setMessageText(JSON.stringify(sample.message, null, 2));
      setMessageError(null);
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
      const parsedMessage = JSON.parse(messageText);
      setMessageText(JSON.stringify(parsedMessage, null, 2));
    } catch {
      // Ignore formatting errors
    }
  }, [workflowsText, messageText]);

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
              <h3>Message</h3>
              {messageError && <span className="editor-error">{messageError}</span>}
            </div>
            <div className="editor-content">
              <JsonEditor
                value={messageText}
                onChange={handleMessageChange}
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
          </div>
          <div className="panel-content">
            <WorkflowVisualizer
              workflows={workflows}
              theme={theme}
            />
          </div>
        </div>
      </main>

      {/* Status Bar */}
      <StatusBar
        workflows={workflows}
        workflowsError={workflowsError}
        messageError={messageError}
        cursorPosition={cursorPosition}
      />
    </div>
  );
}

export default App;
