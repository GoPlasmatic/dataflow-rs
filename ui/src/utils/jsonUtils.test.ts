import { describe, expect, it } from 'vitest';
import { findPathLineNumbers } from './jsonUtils';

describe('findPathLineNumbers', () => {
  it('finds line numbers for nested object keys, array elements, and keys after arrays', () => {
    const doc = JSON.stringify(
      {
        data: {
          tags: ['alpha', 'beta'],
          items: [{ name: 'first' }, { name: 'second' }],
          after: 42,
        },
      },
      null,
      2,
    );
    const lines = doc.split('\n');
    const lineOf = (needle: string) => lines.findIndex((l) => l.includes(needle)) + 1;

    expect(findPathLineNumbers(doc, ['data.tags.0', 'data.tags.1'])).toEqual([
      lineOf('"alpha"'),
      lineOf('"beta"'),
    ]);
    expect(findPathLineNumbers(doc, ['data.items.1.name'])).toEqual([lineOf('"second"')]);
    expect(findPathLineNumbers(doc, ['data.after'])).toEqual([lineOf('"after"')]);
  });
});
