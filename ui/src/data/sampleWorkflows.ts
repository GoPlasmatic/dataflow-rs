import type { Workflow } from '../types';

/**
 * Sample workflows demonstrating progressive complexity
 * All workflows follow the recommended pattern: parse_json first to load payload into data context
 * Uses built-in functions: parse_json, map, validation
 *
 * Examples build on each other:
 * 1. Hello Transform    — basic field mapping & string concatenation
 * 2. Form Validation    — validation rules with error accumulation
 * 3. Invoice Calculator — sequential task dependencies & math
 * 4. Message Router     — multiple workflows with metadata conditions
 * 5. E-Commerce Pipeline — full realistic pipeline with folders & audit
 */
export const SAMPLE_WORKFLOWS: Record<string, { workflows: Workflow[]; payload: object }> = {
  'Hello Transform': {
    workflows: [
      {
        id: 'hello-transform',
        name: 'Hello Transform',
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
            id: 'map-greeting',
            name: 'Create Greeting',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.display_name', logic: { cat: [{ var: 'data.input.first_name' }, ' ', { var: 'data.input.last_name' }] } },
                  { path: 'data.greeting', logic: { cat: ['Hello, ', { var: 'data.input.first_name' }, '! Welcome aboard as ', { var: 'data.input.role' }, '.'] } },
                  { path: 'data.role', logic: { var: 'data.input.role' } },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: { first_name: 'Alice', last_name: 'Chen', role: 'Engineer' },
  },

  'Form Validation': {
    workflows: [
      {
        id: 'form-validation',
        name: 'Form Validation',
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
            id: 'validate-fields',
            name: 'Validate Fields',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '>=': [{ var: 'data.input.age' }, 18] }, message: 'Must be at least 18 years old' },
                  { logic: { '!!': { var: 'data.input.email' } }, message: 'Email is required' },
                  { logic: { '>=': [{ strlen: { var: 'data.input.username' } }, 3] }, message: 'Username must be at least 3 characters' },
                ],
              },
            },
          },
          {
            id: 'map-profile',
            name: 'Build Verified Profile',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.profile.username', logic: { var: 'data.input.username' } },
                  { path: 'data.profile.email', logic: { var: 'data.input.email' } },
                  { path: 'data.profile.age', logic: { var: 'data.input.age' } },
                  { path: 'data.profile.verified', logic: true },
                  { path: 'data.profile.display', logic: { cat: [{ var: 'data.input.username' }, ' <', { var: 'data.input.email' }, '>'] } },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: { username: 'alice_dev', email: 'alice@example.com', age: 25 },
  },

  'Invoice Calculator': {
    workflows: [
      {
        id: 'invoice-calc',
        name: 'Invoice Calculator',
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
            id: 'extract-items',
            name: 'Extract Line Items',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.items', logic: { var: 'data.input.items' } },
                  { path: 'data.tax_rate', logic: { var: 'data.input.tax_rate' } },
                  { path: 'data.customer', logic: { var: 'data.input.customer' } },
                ],
              },
            },
          },
          {
            id: 'calc-subtotal',
            name: 'Calculate Subtotal',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.item_1_total', logic: { '*': [{ var: 'data.items.0.qty' }, { var: 'data.items.0.price' }] } },
                  { path: 'data.item_2_total', logic: { '*': [{ var: 'data.items.1.qty' }, { var: 'data.items.1.price' }] } },
                  { path: 'data.subtotal', logic: { '+': [{ '*': [{ var: 'data.items.0.qty' }, { var: 'data.items.0.price' }] }, { '*': [{ var: 'data.items.1.qty' }, { var: 'data.items.1.price' }] }] } },
                ],
              },
            },
          },
          {
            id: 'apply-discount',
            name: 'Apply Discount (if > $200)',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.discount_pct', logic: { if: [{ '>': [{ var: 'data.subtotal' }, 200] }, 0.10, 0] } },
                  { path: 'data.discount_amount', logic: { '*': [{ var: 'data.subtotal' }, { if: [{ '>': [{ var: 'data.subtotal' }, 200] }, 0.10, 0] }] } },
                  { path: 'data.after_discount', logic: { '-': [{ var: 'data.subtotal' }, { '*': [{ var: 'data.subtotal' }, { if: [{ '>': [{ var: 'data.subtotal' }, 200] }, 0.10, 0] }] }] } },
                ],
              },
            },
          },
          {
            id: 'calc-total',
            name: 'Calculate Tax & Grand Total',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.tax', logic: { '*': [{ var: 'data.after_discount' }, { var: 'data.tax_rate' }] } },
                  { path: 'data.grand_total', logic: { '+': [{ var: 'data.after_discount' }, { '*': [{ var: 'data.after_discount' }, { var: 'data.tax_rate' }] }] } },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: {
      customer: 'Acme Corp',
      tax_rate: 0.08,
      items: [
        { name: 'Widget A', qty: 5, price: 29.99 },
        { name: 'Widget B', qty: 3, price: 49.99 },
      ],
    },
  },

  'Message Router': {
    workflows: [
      {
        id: 'parse-input',
        name: 'Parse & Classify',
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
            id: 'set-metadata',
            name: 'Set Message Type in Metadata',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'metadata.message_type', logic: { var: 'data.input.type' } },
                  { path: 'data.title', logic: { var: 'data.input.title' } },
                  { path: 'data.body', logic: { var: 'data.input.body' } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'route-alert',
        name: 'Alert Handler',
        priority: 1,
        condition: { '==': [{ var: 'metadata.message_type' }, 'alert'] },
        tasks: [
          {
            id: 'format-alert',
            name: 'Format Alert',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.formatted', logic: { cat: ['⚠ ALERT [', { var: 'data.input.severity' }, ']: ', { var: 'data.title' }] } },
                  { path: 'data.severity', logic: { var: 'data.input.severity' } },
                  { path: 'data.channel', logic: 'pager' },
                ],
              },
            },
          },
          {
            id: 'validate-alert',
            name: 'Validate Alert',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '!!': { var: 'data.severity' } }, message: 'Alert must have a severity level' },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'route-notification',
        name: 'Notification Handler',
        priority: 1,
        condition: { '==': [{ var: 'metadata.message_type' }, 'notification'] },
        tasks: [
          {
            id: 'format-notification',
            name: 'Format Notification',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.formatted', logic: { cat: ['📬 ', { var: 'data.title' }, ': ', { var: 'data.body' }] } },
                  { path: 'data.channel', logic: 'email' },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: { type: 'alert', severity: 'high', title: 'CPU Overload', body: 'Server cpu-3 at 98% utilization' },
  },

  'E-Commerce Pipeline': {
    workflows: [
      {
        id: 'intake-parse',
        name: 'Parse Order',
        path: 'intake',
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
            id: 'extract-data',
            name: 'Extract Order Data',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.customer.name', logic: { var: 'data.input.customer.name' } },
                  { path: 'data.customer.email', logic: { var: 'data.input.customer.email' } },
                  { path: 'data.customer.tier', logic: { var: 'data.input.customer.tier' } },
                  { path: 'data.items', logic: { var: 'data.input.items' } },
                  { path: 'data.shipping_address', logic: { var: 'data.input.shipping_address' } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'intake-validate',
        name: 'Validate Order',
        path: 'intake',
        priority: 1,
        tasks: [
          {
            id: 'validate-customer',
            name: 'Validate Customer',
            function: {
              name: 'validation',
              input: {
                rules: [
                  { logic: { '!!': { var: 'data.customer.name' } }, message: 'Customer name is required' },
                  { logic: { '!!': { var: 'data.customer.email' } }, message: 'Customer email is required' },
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
                  { logic: { '!!': { var: 'data.items.0' } }, message: 'At least one item is required' },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'processing-pricing',
        name: 'Calculate Pricing',
        path: 'processing',
        priority: 2,
        tasks: [
          {
            id: 'calc-line-totals',
            name: 'Calculate Line Totals',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.line_totals.0', logic: { '*': [{ var: 'data.items.0.qty' }, { var: 'data.items.0.price' }] } },
                  { path: 'data.line_totals.1', logic: { '*': [{ var: 'data.items.1.qty' }, { var: 'data.items.1.price' }] } },
                  { path: 'data.line_totals.2', logic: { '*': [{ var: 'data.items.2.qty' }, { var: 'data.items.2.price' }] } },
                ],
              },
            },
          },
          {
            id: 'calc-subtotal',
            name: 'Calculate Subtotal',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.pricing.subtotal', logic: { '+': [{ var: 'data.line_totals.0' }, { var: 'data.line_totals.1' }, { var: 'data.line_totals.2' }] } },
                  { path: 'data.pricing.tax_rate', logic: 0.08 },
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
                  { path: 'data.pricing.tax', logic: { '*': [{ var: 'data.pricing.subtotal' }, { var: 'data.pricing.tax_rate' }] } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'processing-discounts',
        name: 'Apply Discounts',
        path: 'processing',
        priority: 3,
        tasks: [
          {
            id: 'tiered-discount',
            name: 'Tiered Discount',
            function: {
              name: 'map',
              input: {
                mappings: [
                  {
                    path: 'data.pricing.discount_pct',
                    logic: {
                      if: [
                        { '>': [{ var: 'data.pricing.subtotal' }, 500] }, 0.15,
                        { '>': [{ var: 'data.pricing.subtotal' }, 200] }, 0.10,
                        { '>': [{ var: 'data.pricing.subtotal' }, 100] }, 0.05,
                        0,
                      ],
                    },
                  },
                  {
                    path: 'data.pricing.discount',
                    logic: {
                      '*': [
                        { var: 'data.pricing.subtotal' },
                        {
                          if: [
                            { '>': [{ var: 'data.pricing.subtotal' }, 500] }, 0.15,
                            { '>': [{ var: 'data.pricing.subtotal' }, 200] }, 0.10,
                            { '>': [{ var: 'data.pricing.subtotal' }, 100] }, 0.05,
                            0,
                          ],
                        },
                      ],
                    },
                  },
                ],
              },
            },
          },
          {
            id: 'calc-after-discount',
            name: 'Subtotal After Discount',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.pricing.after_discount', logic: { '-': [{ var: 'data.pricing.subtotal' }, { var: 'data.pricing.discount' }] } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'processing-shipping',
        name: 'Shipping',
        path: 'processing',
        priority: 4,
        tasks: [
          {
            id: 'determine-shipping',
            name: 'Determine Shipping',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.shipping.method', logic: { if: [{ '>': [{ var: 'data.pricing.after_discount' }, 150] }, 'express', 'standard'] } },
                  { path: 'data.shipping.cost', logic: { if: [{ '>': [{ var: 'data.pricing.after_discount' }, 150] }, 0, 9.99] } },
                ],
              },
            },
          },
          {
            id: 'calc-grand-total',
            name: 'Calculate Grand Total',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.pricing.grand_total', logic: { '+': [{ var: 'data.pricing.after_discount' }, { var: 'data.pricing.tax' }, { var: 'data.shipping.cost' }] } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'output-format',
        name: 'Format Response',
        path: 'output',
        priority: 5,
        tasks: [
          {
            id: 'build-response',
            name: 'Build Order Response',
            function: {
              name: 'map',
              input: {
                mappings: [
                  { path: 'data.response.order_id', logic: { cat: ['ORD-', { var: 'data.customer.name' }, '-001'] } },
                  { path: 'data.response.status', logic: 'confirmed' },
                  { path: 'data.response.customer', logic: { var: 'data.customer.name' } },
                  { path: 'data.response.subtotal', logic: { var: 'data.pricing.subtotal' } },
                  { path: 'data.response.discount', logic: { var: 'data.pricing.discount' } },
                  { path: 'data.response.tax', logic: { var: 'data.pricing.tax' } },
                  { path: 'data.response.shipping', logic: { var: 'data.shipping.cost' } },
                  { path: 'data.response.grand_total', logic: { var: 'data.pricing.grand_total' } },
                  { path: 'data.response.shipping_method', logic: { var: 'data.shipping.method' } },
                ],
              },
            },
          },
        ],
      },
      {
        id: 'output-audit',
        name: 'Audit Trail',
        path: 'output',
        priority: 6,
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
                  { path: 'data.audit.customer_email', logic: { var: 'data.customer.email' } },
                  { path: 'data.audit.total_amount', logic: { var: 'data.pricing.grand_total' } },
                  { path: 'data.audit.item_count', logic: 3 },
                ],
              },
            },
          },
        ],
      },
    ],
    payload: {
      customer: {
        name: 'Acme Corp',
        email: 'orders@acme.com',
        tier: 'gold',
      },
      items: [
        { name: 'Laptop Stand', qty: 2, price: 49.99 },
        { name: 'USB-C Hub', qty: 1, price: 79.99 },
        { name: 'Webcam HD', qty: 3, price: 34.99 },
      ],
      shipping_address: {
        street: '123 Innovation Dr',
        city: 'San Francisco',
        state: 'CA',
        zip: '94105',
      },
    },
  },
};
