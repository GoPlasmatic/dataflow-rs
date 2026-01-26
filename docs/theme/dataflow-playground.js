/**
 * Dataflow Playground - Interactive workflow processing
 * Provides inline "Try It" widgets and a full playground page
 */

// WASM module state
let wasmReady = false;
let wasmModule = null;
let initPromise = null;

// Get the base path for loading WASM files
function getBasePath() {
    const scripts = document.getElementsByTagName('script');
    for (let script of scripts) {
        if (script.src && script.src.includes('dataflow-playground')) {
            return script.src.replace(/theme\/dataflow-playground[^/]*\.js.*$/, '');
        }
    }
    // Fallback: try to detect from current URL by looking for path_to_root
    // mdBook sets this variable in each page
    if (typeof path_to_root !== 'undefined') {
        return window.location.href.replace(/[^/]*$/, '') + path_to_root;
    }
    // Final fallback: try to detect from current URL
    const path = window.location.pathname;
    const parts = path.split('/');
    parts.pop();
    return window.location.origin + parts.join('/') + '/';
}

// Initialize WASM module
async function initWasm() {
    if (initPromise) return initPromise;

    initPromise = (async () => {
        try {
            const basePath = getBasePath();
            const wasmJsUrl = basePath + 'wasm/dataflow_wasm.js';

            // Dynamic import of WASM JS module
            const module = await import(wasmJsUrl);

            // Initialize WASM - the default export initializes the module
            await module.default();

            wasmModule = module;
            wasmReady = true;
            console.log('Dataflow WASM initialized successfully');
            return true;
        } catch (error) {
            console.error('Failed to initialize Dataflow WASM:', error);
            wasmReady = false;
            return false;
        }
    })();

    return initPromise;
}

// Process message through workflows
async function processMessage(workflowsJson, messageJson) {
    if (!wasmReady || !wasmModule) {
        throw new Error('WASM module not initialized');
    }

    // Parse the message to extract data and metadata
    const msg = JSON.parse(messageJson);
    const data = JSON.stringify(msg.data || {});
    const metadata = JSON.stringify(msg.metadata || {});

    // Create a message using the WASM helper
    const fullMessage = wasmModule.create_message(data, metadata);

    // Process using one-off convenience function
    const result = await wasmModule.process_message(workflowsJson, fullMessage);
    return result;
}

// Format JSON for display
function formatJson(str) {
    try {
        const obj = JSON.parse(str);
        return JSON.stringify(obj, null, 2);
    } catch {
        return str;
    }
}

// Validate JSON string
function isValidJson(str) {
    try {
        JSON.parse(str);
        return true;
    } catch {
        return false;
    }
}

// JSON syntax highlighting
function highlightJson(str) {
    // Escape HTML entities
    const escaped = str
        .replace(/&/g, '&amp;')
        .replace(/</g, '&lt;')
        .replace(/>/g, '&gt;');

    // Apply syntax highlighting
    return escaped
        // Strings (including keys in quotes)
        .replace(/"([^"\\]|\\.)*"/g, (match) => {
            return `<span class="json-string">${match}</span>`;
        })
        // Numbers
        .replace(/\b(-?\d+\.?\d*([eE][+-]?\d+)?)\b/g, '<span class="json-number">$1</span>')
        // Booleans
        .replace(/\b(true|false)\b/g, '<span class="json-boolean">$1</span>')
        // Null
        .replace(/\bnull\b/g, '<span class="json-null">null</span>')
        // Brackets and braces
        .replace(/([{}\[\]])/g, '<span class="json-bracket">$1</span>')
        // Highlight keys (strings followed by :)
        .replace(/<span class="json-string">("([^"\\]|\\.)*")<\/span>(\s*:)/g,
            '<span class="json-key">$1</span>$3');
}

// Sync scroll between textarea and highlight layer
function syncScroll(textarea, highlight) {
    highlight.scrollTop = textarea.scrollTop;
    highlight.scrollLeft = textarea.scrollLeft;
}

// Create an inline "Try It" widget
function createWidget(container) {
    const workflows = container.dataset.workflows || '[]';
    const message = container.dataset.message || '{"data":{},"metadata":{}}';
    const originalWorkflows = workflows;
    const originalMessage = message;

    const formattedWorkflows = formatJson(workflows);
    const formattedMessage = formatJson(message);

    container.innerHTML = `
        <div class="playground-widget-inner">
            <div class="playground-header">
                <span class="playground-title">Try It</span>
                <div class="playground-actions">
                    <button class="playground-btn playground-reset" title="Reset to original">Reset</button>
                    <button class="playground-btn playground-run" title="Process (Ctrl+Enter)">Process</button>
                </div>
            </div>
            <div class="playground-body">
                <div class="playground-inputs">
                    <div class="playground-editor-section">
                        <label>Workflows</label>
                        <div class="playground-editor-container">
                            <div class="playground-highlight playground-workflows-highlight" aria-hidden="true">${highlightJson(formattedWorkflows)}</div>
                            <textarea class="playground-workflows" spellcheck="false">${formattedWorkflows}</textarea>
                        </div>
                    </div>
                    <div class="playground-editor-section">
                        <label>Message</label>
                        <div class="playground-editor-container">
                            <div class="playground-highlight playground-message-highlight" aria-hidden="true">${highlightJson(formattedMessage)}</div>
                            <textarea class="playground-message" spellcheck="false">${formattedMessage}</textarea>
                        </div>
                    </div>
                </div>
                <div class="playground-output">
                    <label>Result</label>
                    <div class="playground-result"></div>
                </div>
            </div>
        </div>
    `;

    const workflowsInput = container.querySelector('.playground-workflows');
    const workflowsHighlight = container.querySelector('.playground-workflows-highlight');
    const messageInput = container.querySelector('.playground-message');
    const messageHighlight = container.querySelector('.playground-message-highlight');
    const resultDiv = container.querySelector('.playground-result');
    const runBtn = container.querySelector('.playground-run');
    const resetBtn = container.querySelector('.playground-reset');

    // Update highlighting on input
    function updateWorkflowsHighlight() {
        workflowsHighlight.innerHTML = highlightJson(workflowsInput.value);
    }

    function updateMessageHighlight() {
        messageHighlight.innerHTML = highlightJson(messageInput.value);
    }

    // Run processing
    async function run() {
        const workflowsStr = workflowsInput.value.trim();
        const messageStr = messageInput.value.trim();

        // Validate JSON
        if (!isValidJson(workflowsStr)) {
            resultDiv.className = 'playground-result error';
            resultDiv.textContent = 'Invalid workflows JSON';
            return;
        }
        if (!isValidJson(messageStr)) {
            resultDiv.className = 'playground-result error';
            resultDiv.textContent = 'Invalid message JSON';
            return;
        }

        try {
            resultDiv.className = 'playground-result loading';
            resultDiv.textContent = 'Processing...';

            const result = await processMessage(workflowsStr, messageStr);
            resultDiv.className = 'playground-result success';
            resultDiv.innerHTML = formatResultWithSections(result);
        } catch (error) {
            resultDiv.className = 'playground-result error';
            resultDiv.textContent = 'Error: ' + error.message;
        }
    }

    // Reset to original values
    function reset() {
        workflowsInput.value = formatJson(originalWorkflows);
        messageInput.value = formatJson(originalMessage);
        updateWorkflowsHighlight();
        updateMessageHighlight();
        resultDiv.className = 'playground-result';
        resultDiv.textContent = '';
    }

    // Event listeners
    runBtn.addEventListener('click', run);
    resetBtn.addEventListener('click', reset);

    // Input events for highlighting
    workflowsInput.addEventListener('input', updateWorkflowsHighlight);
    messageInput.addEventListener('input', updateMessageHighlight);

    // Scroll sync
    workflowsInput.addEventListener('scroll', () => syncScroll(workflowsInput, workflowsHighlight));
    messageInput.addEventListener('scroll', () => syncScroll(messageInput, messageHighlight));

    // Keyboard shortcut: Ctrl/Cmd + Enter to run
    function handleKeydown(e) {
        e.stopPropagation();
        if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
            e.preventDefault();
            run();
        }
    }
    workflowsInput.addEventListener('keydown', handleKeydown);
    messageInput.addEventListener('keydown', handleKeydown);

    // Auto-run on initial load
    if (wasmReady) {
        run();
    }
}

// Format result with collapsible sections for audit trail
function formatResultWithSections(resultStr) {
    try {
        const result = JSON.parse(resultStr);
        let html = '';

        // Context (data, metadata, temp_data)
        if (result.context) {
            html += '<div class="result-section">';
            html += '<div class="result-section-header">Context</div>';
            html += '<pre class="result-json">' + highlightJson(JSON.stringify(result.context, null, 2)) + '</pre>';
            html += '</div>';
        }

        // Errors
        if (result.errors && result.errors.length > 0) {
            html += '<div class="result-section result-errors">';
            html += '<div class="result-section-header">Errors (' + result.errors.length + ')</div>';
            html += '<pre class="result-json">' + highlightJson(JSON.stringify(result.errors, null, 2)) + '</pre>';
            html += '</div>';
        }

        // Audit Trail
        if (result.audit_trail && result.audit_trail.length > 0) {
            html += '<div class="result-section result-audit">';
            html += '<div class="result-section-header">Audit Trail (' + result.audit_trail.length + ' entries)</div>';
            html += '<pre class="result-json">' + highlightJson(JSON.stringify(result.audit_trail, null, 2)) + '</pre>';
            html += '</div>';
        }

        // If no structured result, show raw
        if (!html) {
            html = '<pre class="result-json">' + highlightJson(formatJson(resultStr)) + '</pre>';
        }

        return html;
    } catch {
        return '<pre class="result-json">' + highlightJson(formatJson(resultStr)) + '</pre>';
    }
}

// Initialize all playground widgets on the page
function initPlaygroundWidgets() {
    const widgets = document.querySelectorAll('.playground-widget');
    widgets.forEach(widget => {
        if (!widget.classList.contains('initialized')) {
            createWidget(widget);
            widget.classList.add('initialized');
        }
    });
}

// Create the full playground page
function initFullPlayground() {
    const container = document.getElementById('full-playground');
    if (!container) return;

    // Example templates
    const examples = {
        'Simple Workflow': {
            workflows: '[{"id":"simple","name":"Simple Workflow","tasks":[{"id":"greet","name":"Greet","function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":{"cat":["Hello, ",{"var":"data.name"},"!"]}}]}}}]}]',
            message: '{"data":{"name":"World"},"metadata":{}}'
        },
        'Data Mapping': {
            workflows: '[{"id":"mapping","name":"Data Mapping","tasks":[{"id":"map_fields","name":"Map Fields","function":{"name":"map","input":{"mappings":[{"path":"data.full_name","logic":{"cat":[{"var":"data.first_name"}," ",{"var":"data.last_name"}]}},{"path":"data.is_valid_email","logic":{"in":["@",{"var":"data.email"}]}}]}}}]}]',
            message: '{"data":{"first_name":"John","last_name":"Doe","email":"john@example.com"},"metadata":{}}'
        },
        'Validation Rules': {
            workflows: '[{"id":"validate","name":"Validation","tasks":[{"id":"check","name":"Check Data","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.email"}]},"message":"Email is required"},{"logic":{">":[{"var":"data.age"},0]},"message":"Age must be positive"},{"logic":{"in":[{"var":"data.status"},["active","pending"]]},"message":"Invalid status"}]}}}]}]',
            message: '{"data":{"name":"John","age":-5,"status":"unknown"},"metadata":{}}'
        },
        'Conditional Task': {
            workflows: '[{"id":"conditional","name":"Conditional Workflow","tasks":[{"id":"premium_greeting","name":"Premium Greeting","condition":{"==":[{"var":"data.tier"},"premium"]},"function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":"Welcome, VIP member!"},{"path":"data.discount","logic":20}]}}},{"id":"standard_greeting","name":"Standard Greeting","condition":{"==":[{"var":"data.tier"},"standard"]},"function":{"name":"map","input":{"mappings":[{"path":"data.greeting","logic":"Welcome!"},{"path":"data.discount","logic":5}]}}}]}]',
            message: '{"data":{"name":"John","tier":"premium"},"metadata":{}}'
        },
        'Multi-Workflow': {
            workflows: '[{"id":"enrich","name":"Enrich Data","priority":1,"tasks":[{"id":"add_timestamp","name":"Add Timestamp","function":{"name":"map","input":{"mappings":[{"path":"temp_data.processed_at","logic":"2024-01-01T00:00:00Z"}]}}}]},{"id":"transform","name":"Transform Data","priority":2,"tasks":[{"id":"build_output","name":"Build Output","function":{"name":"map","input":{"mappings":[{"path":"data.output","logic":{"cat":["Processed: ",{"var":"data.input"}," at ",{"var":"temp_data.processed_at"}]}}]}}}]}]',
            message: '{"data":{"input":"test data"},"metadata":{}}'
        },
        'Error Handling': {
            workflows: '[{"id":"resilient","name":"Resilient Workflow","continue_on_error":true,"tasks":[{"id":"validate","name":"Validate","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.required_field"}]},"message":"Required field missing"}]}}},{"id":"process","name":"Process Anyway","function":{"name":"map","input":{"mappings":[{"path":"data.processed","logic":true}]}}}]}]',
            message: '{"data":{"other_field":"value"},"metadata":{}}'
        },
        'Arithmetic': {
            workflows: '[{"id":"calc","name":"Calculations","tasks":[{"id":"compute","name":"Compute","function":{"name":"map","input":{"mappings":[{"path":"data.subtotal","logic":{"*":[{"var":"data.price"},{"var":"data.quantity"}]}},{"path":"data.tax","logic":{"*":[{"var":"data.subtotal"},0.1]}},{"path":"data.total","logic":{"+":[{"var":"data.subtotal"},{"var":"data.tax"}]}}]}}}]}]',
            message: '{"data":{"price":25,"quantity":4},"metadata":{}}'
        },
        'Array Processing': {
            workflows: '[{"id":"arrays","name":"Array Processing","tasks":[{"id":"process","name":"Process Arrays","function":{"name":"map","input":{"mappings":[{"path":"data.count","logic":{"reduce":[{"var":"data.items"},{"+": [{"var":"accumulator"},1]},0]}},{"path":"data.sum","logic":{"reduce":[{"var":"data.values"},{"+": [{"var":"accumulator"},{"var":"current"}]},0]}},{"path":"data.has_special","logic":{"some":[{"var":"data.items"},{"==":[{"var":""},"special"]}]}}]}}}]}]',
            message: '{"data":{"items":["a","special","b"],"values":[10,20,30]},"metadata":{}}'
        },
        'Workflow Condition': {
            workflows: '[{"id":"user_only","name":"User Workflow","condition":{"==":[{"var":"metadata.type"},"user"]},"tasks":[{"id":"greet","function":{"name":"map","input":{"mappings":[{"path":"data.message","logic":"This is a user message"}]}}}]},{"id":"system_only","name":"System Workflow","condition":{"==":[{"var":"metadata.type"},"system"]},"tasks":[{"id":"log","function":{"name":"map","input":{"mappings":[{"path":"data.message","logic":"This is a system message"}]}}}]}]',
            message: '{"data":{},"metadata":{"type":"user"}}'
        },
        'Complete Example': {
            workflows: '[{"id":"process_order","name":"Process Order","priority":1,"tasks":[{"id":"validate_order","name":"Validate Order","function":{"name":"validation","input":{"rules":[{"logic":{"!!":[{"var":"data.customer_id"}]},"message":"Customer ID required"},{"logic":{">":[{"var":"data.amount"},0]},"message":"Amount must be positive"}]}}},{"id":"calculate_total","name":"Calculate Total","function":{"name":"map","input":{"mappings":[{"path":"data.tax","logic":{"*":[{"var":"data.amount"},0.1]}},{"path":"data.total","logic":{"+":[{"var":"data.amount"},{"var":"data.tax"}]}},{"path":"data.status","logic":"processed"},{"path":"temp_data.order_time","logic":"2024-01-01T12:00:00Z"}]}}},{"id":"set_metadata","name":"Set Metadata","function":{"name":"map","input":{"mappings":[{"path":"metadata.processed_at","logic":{"var":"temp_data.order_time"}},{"path":"metadata.processor","logic":"workflow-engine"}]}}}]}]',
            message: '{"data":{"customer_id":"cust-123","amount":99.99,"items":["item1","item2"]},"metadata":{"source":"api"}}'
        }
    };

    const firstExample = Object.keys(examples)[0];
    const formattedWorkflows = formatJson(examples[firstExample].workflows);
    const formattedMessage = formatJson(examples[firstExample].message);

    container.innerHTML = `
        <div class="full-playground-container">
            <div class="full-playground-header">
                <span class="full-playground-title">Dataflow Playground</span>
                <div class="full-playground-controls">
                    <select class="playground-examples">
                        <option value="">Load Example...</option>
                        ${Object.keys(examples).map(name => `<option value="${name}">${name}</option>`).join('')}
                    </select>
                    <button class="playground-btn playground-reset" title="Format JSON">Format</button>
                    <button class="playground-btn playground-reset playground-clear" title="Clear all">Clear</button>
                    <button class="playground-btn playground-run" title="Process (Ctrl+Enter)">Process</button>
                </div>
            </div>
            <div class="full-playground-body">
                <div class="full-playground-inputs">
                    <div class="full-playground-editor-section">
                        <label>Workflows (JSON Array)</label>
                        <div class="full-playground-editor-container">
                            <div class="full-playground-highlight full-playground-workflows-highlight" aria-hidden="true">${highlightJson(formattedWorkflows)}</div>
                            <textarea class="full-playground-workflows" spellcheck="false" placeholder='[{"id": "workflow", "tasks": [...]}]'>${formattedWorkflows}</textarea>
                        </div>
                    </div>
                    <div class="full-playground-editor-section">
                        <label>Message</label>
                        <div class="full-playground-editor-container">
                            <div class="full-playground-highlight full-playground-message-highlight" aria-hidden="true">${highlightJson(formattedMessage)}</div>
                            <textarea class="full-playground-message" spellcheck="false" placeholder='{"data": {}, "metadata": {}}'>${formattedMessage}</textarea>
                        </div>
                    </div>
                </div>
                <div class="full-playground-output">
                    <label>Result</label>
                    <div class="full-playground-result"></div>
                </div>
            </div>
        </div>
    `;

    const workflowsInput = container.querySelector('.full-playground-workflows');
    const workflowsHighlight = container.querySelector('.full-playground-workflows-highlight');
    const messageInput = container.querySelector('.full-playground-message');
    const messageHighlight = container.querySelector('.full-playground-message-highlight');
    const resultDiv = container.querySelector('.full-playground-result');
    const runBtn = container.querySelector('.playground-run');
    const formatBtn = container.querySelector('.playground-reset:not(.playground-clear)');
    const clearBtn = container.querySelector('.playground-clear');
    const examplesSelect = container.querySelector('.playground-examples');

    // Update highlighting on input
    function updateWorkflowsHighlight() {
        workflowsHighlight.innerHTML = highlightJson(workflowsInput.value);
    }

    function updateMessageHighlight() {
        messageHighlight.innerHTML = highlightJson(messageInput.value);
    }

    // Run processing
    async function run() {
        const workflowsStr = workflowsInput.value.trim();
        const messageStr = messageInput.value.trim();

        if (!workflowsStr || !messageStr) {
            resultDiv.className = 'full-playground-result error';
            resultDiv.textContent = 'Please enter workflows and message';
            return;
        }

        if (!isValidJson(workflowsStr)) {
            resultDiv.className = 'full-playground-result error';
            resultDiv.textContent = 'Invalid workflows JSON';
            return;
        }

        if (!isValidJson(messageStr)) {
            resultDiv.className = 'full-playground-result error';
            resultDiv.textContent = 'Invalid message JSON';
            return;
        }

        try {
            resultDiv.className = 'full-playground-result loading';
            resultDiv.textContent = 'Processing...';

            const result = await processMessage(workflowsStr, messageStr);
            resultDiv.className = 'full-playground-result success';
            resultDiv.innerHTML = formatResultWithSections(result);
        } catch (error) {
            resultDiv.className = 'full-playground-result error';
            resultDiv.textContent = 'Error: ' + error.message;
        }
    }

    // Format JSON in editors
    function format() {
        try {
            workflowsInput.value = formatJson(workflowsInput.value);
            updateWorkflowsHighlight();
        } catch {}
        try {
            messageInput.value = formatJson(messageInput.value);
            updateMessageHighlight();
        } catch {}
    }

    // Clear all
    function clear() {
        workflowsInput.value = '[]';
        messageInput.value = '{"data": {}, "metadata": {}}';
        updateWorkflowsHighlight();
        updateMessageHighlight();
        resultDiv.className = 'full-playground-result';
        resultDiv.textContent = '';
        examplesSelect.value = '';
    }

    // Load example
    function loadExample() {
        const name = examplesSelect.value;
        if (name && examples[name]) {
            workflowsInput.value = formatJson(examples[name].workflows);
            messageInput.value = formatJson(examples[name].message);
            updateWorkflowsHighlight();
            updateMessageHighlight();
            run();
        }
    }

    // Event listeners
    runBtn.addEventListener('click', run);
    formatBtn.addEventListener('click', format);
    clearBtn.addEventListener('click', clear);
    examplesSelect.addEventListener('change', loadExample);

    // Input events for highlighting
    workflowsInput.addEventListener('input', updateWorkflowsHighlight);
    messageInput.addEventListener('input', updateMessageHighlight);

    // Scroll sync
    workflowsInput.addEventListener('scroll', () => syncScroll(workflowsInput, workflowsHighlight));
    messageInput.addEventListener('scroll', () => syncScroll(messageInput, messageHighlight));

    // Keyboard shortcut
    function handleKeydown(e) {
        e.stopPropagation();
        if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
            e.preventDefault();
            run();
        }
    }
    workflowsInput.addEventListener('keydown', handleKeydown);
    messageInput.addEventListener('keydown', handleKeydown);

    // Auto-run on initial load
    if (wasmReady) {
        run();
    }
}

// Initialize on page load
document.addEventListener('DOMContentLoaded', async () => {
    // Show loading state for widgets
    document.querySelectorAll('.playground-widget').forEach(widget => {
        widget.innerHTML = '<div class="playground-loading">Loading playground...</div>';
    });

    const fullPlayground = document.getElementById('full-playground');
    if (fullPlayground) {
        fullPlayground.innerHTML = '<div class="playground-loading">Loading playground...</div>';
    }

    // Initialize WASM
    const success = await initWasm();

    if (success) {
        // Initialize widgets
        initPlaygroundWidgets();
        initFullPlayground();
    } else {
        // Show error state
        document.querySelectorAll('.playground-widget').forEach(widget => {
            widget.innerHTML = '<div class="playground-error">Failed to load playground. Please refresh the page.</div>';
        });
        if (fullPlayground) {
            fullPlayground.innerHTML = '<div class="playground-error">Failed to load playground. Please refresh the page.</div>';
        }
    }
});

// Re-initialize widgets when page content changes (for mdBook's navigation)
if (typeof window !== 'undefined') {
    // MutationObserver to detect page changes
    const observer = new MutationObserver((mutations) => {
        if (wasmReady) {
            initPlaygroundWidgets();
            initFullPlayground();
        }
    });

    // Start observing when DOM is ready
    document.addEventListener('DOMContentLoaded', () => {
        const content = document.getElementById('content');
        if (content) {
            observer.observe(content, { childList: true, subtree: true });
        }
    });
}
