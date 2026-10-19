import { useState, useCallback } from 'react';

export function useExpandedState(initialExpanded: string[] = []) {
  const [expandedIds, setExpandedIds] = useState<Set<string>>(
    new Set(initialExpanded)
  );

  const isExpanded = useCallback(
    (id: string) => expandedIds.has(id),
    [expandedIds]
  );

  const toggle = useCallback((id: string) => {
    setExpandedIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  }, []);

  const expandAll = useCallback((ids: string[]) => {
    setExpandedIds(new Set(ids));
  }, []);

  const collapseAll = useCallback(() => {
    setExpandedIds(new Set());
  }, []);

  return {
    isExpanded,
    toggle,
    expandAll,
    collapseAll,
  };
}
