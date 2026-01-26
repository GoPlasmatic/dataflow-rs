/**
 * Find line numbers for JSON paths in a formatted JSON string
 */
export function findPathLineNumbers(jsonString: string, paths: string[]): number[] {
  if (!paths || paths.length === 0) return [];

  const lines = jsonString.split('\n');
  const lineNumbers: number[] = [];

  for (const path of paths) {
    // Convert path like "data.user.name" to search for key patterns
    const pathParts = path.split('.');
    const lastKey = pathParts[pathParts.length - 1];

    // Search for the key in the JSON - look for "key": pattern
    const keyPattern = new RegExp(`^\\s*"${lastKey}"\\s*:`);

    for (let i = 0; i < lines.length; i++) {
      if (keyPattern.test(lines[i])) {
        // Verify this is the right path by checking parent keys
        if (pathParts.length === 1) {
          lineNumbers.push(i + 1); // Monaco lines are 1-indexed
        } else {
          // Check if parent keys exist in previous lines
          let matchedParents = 0;
          for (let j = i - 1; j >= 0 && matchedParents < pathParts.length - 1; j--) {
            const parentKey = pathParts[pathParts.length - 2 - matchedParents];
            const parentPattern = new RegExp(`^\\s*"${parentKey}"\\s*:`);
            if (parentPattern.test(lines[j])) {
              matchedParents++;
            }
          }
          if (matchedParents >= Math.min(pathParts.length - 1, 2)) {
            lineNumbers.push(i + 1);
          }
        }
      }
    }
  }

  return [...new Set(lineNumbers)]; // Remove duplicates
}
