import { useState, useMemo, useCallback } from 'react';
import type { Workflow } from '../../../types';

interface SearchResult {
  workflows: Workflow[];
  matchedTaskIds: Set<string>;
}

/**
 * Safely stringify a value, returning empty string on failure (e.g., circular references)
 */
function safeStringify(value: unknown): string {
  try {
    return JSON.stringify(value);
  } catch {
    return '';
  }
}

export function useSearch(workflows: Workflow[]) {
  const [searchQuery, setSearchQuery] = useState('');

  const searchResults = useMemo((): SearchResult => {
    const query = searchQuery.toLowerCase().trim();

    if (!query) {
      return {
        workflows,
        matchedTaskIds: new Set(),
      };
    }

    const matchedTaskIds = new Set<string>();
    const filteredWorkflows: Workflow[] = [];

    for (const workflow of workflows) {
      const workflowMatches =
        workflow.id.toLowerCase().includes(query) ||
        workflow.name.toLowerCase().includes(query) ||
        workflow.description?.toLowerCase().includes(query) ||
        safeStringify(workflow.condition).toLowerCase().includes(query);

      const matchingTasks = workflow.tasks.filter((task) => {
        const taskMatches =
          task.id.toLowerCase().includes(query) ||
          task.name.toLowerCase().includes(query) ||
          task.description?.toLowerCase().includes(query) ||
          task.function.name.toLowerCase().includes(query) ||
          safeStringify(task.condition).toLowerCase().includes(query) ||
          safeStringify(task.function.input).toLowerCase().includes(query);

        if (taskMatches) {
          matchedTaskIds.add(task.id);
        }
        return taskMatches;
      });

      if (workflowMatches || matchingTasks.length > 0) {
        filteredWorkflows.push(workflow);
      }
    }

    return {
      workflows: filteredWorkflows,
      matchedTaskIds,
    };
  }, [workflows, searchQuery]);

  const clearSearch = useCallback(() => {
    setSearchQuery('');
  }, []);

  return {
    searchQuery,
    setSearchQuery,
    clearSearch,
    filteredWorkflows: searchResults.workflows,
    matchedTaskIds: searchResults.matchedTaskIds,
    hasActiveSearch: searchQuery.trim().length > 0,
  };
}
