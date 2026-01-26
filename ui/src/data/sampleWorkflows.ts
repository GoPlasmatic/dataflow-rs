import type { Workflow } from '../types';

/**
 * Sample workflows for demonstration
 * All workflows follow the recommended pattern: parse_json first to load payload into data context
 * Uses built-in functions: parse_json, map, validation
 */
export const SAMPLE_WORKFLOWS: Record<string, { workflows: Workflow[]; payload: object }> = {
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
