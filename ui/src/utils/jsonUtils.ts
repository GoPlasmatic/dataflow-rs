/**
 * Find line numbers for JSON paths in a formatted JSON string.
 * Uses a structured walk of the parsed object to produce exact matches.
 */
export function findPathLineNumbers(jsonString: string, paths: string[]): number[] {
  if (!paths || paths.length === 0) return [];

  // Build a set of target paths for fast lookup
  const targetPaths = new Set(paths);

  // Parse the JSON to walk its structure
  let parsed: unknown;
  try {
    parsed = JSON.parse(jsonString);
  } catch {
    return [];
  }

  const lines = jsonString.split('\n');
  const lineNumbers: number[] = [];

  // Walk the object and track line numbers by matching keys in order
  // We scan lines sequentially, tracking our position in the object structure
  const keyStack: string[] = [];
  let lineIndex = 0;

  function currentPath(): string {
    return keyStack.join('.');
  }

  function walkValue(value: unknown): void {
    if (value === null || typeof value !== 'object') {
      // Primitive — the key line was already recorded if needed
      return;
    }

    if (Array.isArray(value)) {
      // For arrays, advance past '[' and walk each element
      // Array indices are represented as numeric path parts
      for (let i = 0; i < value.length; i++) {
        keyStack.push(String(i));
        // Find the next line with content for this array element
        advancePastContent();
        walkValue(value[i]);
        keyStack.pop();
      }
      return;
    }

    // Object: find each key in the lines
    const keys = Object.keys(value);
    for (const key of keys) {
      // Find the line containing this key
      const keyPattern = new RegExp(`^\\s*"${escapeRegex(key)}"\\s*:`);
      while (lineIndex < lines.length) {
        if (keyPattern.test(lines[lineIndex])) {
          keyStack.push(key);
          const path = currentPath();
          if (targetPaths.has(path)) {
            lineNumbers.push(lineIndex + 1); // Monaco lines are 1-indexed
          }
          lineIndex++;
          walkValue((value as Record<string, unknown>)[key]);
          keyStack.pop();
          break;
        }
        lineIndex++;
      }
    }
  }

  function advancePastContent(): void {
    // Skip whitespace/structural lines to position at next content
    while (lineIndex < lines.length) {
      const trimmed = lines[lineIndex].trim();
      if (trimmed && trimmed !== ']' && trimmed !== '}' && trimmed !== '],' && trimmed !== '},') {
        return;
      }
      lineIndex++;
    }
  }

  walkValue(parsed);

  return [...new Set(lineNumbers)];
}

function escapeRegex(str: string): string {
  return str.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}
