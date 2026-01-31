import type { Workflow } from '../../../types';
import { NODE_IDS } from '../constants';

/**
 * Represents a folder node in the tree hierarchy
 */
export interface FolderTreeNode {
  /** Segment name (e.g., "processing") */
  name: string;
  /** Full path (e.g., "orders/processing") */
  fullPath: string;
  /** Child folders */
  folders: Map<string, FolderTreeNode>;
  /** Workflows directly in this folder */
  workflows: Workflow[];
  /** Total workflow count including nested folders */
  totalWorkflowCount: number;
}

/**
 * Represents the root-level folder tree
 */
export interface FolderTree {
  /** Root-level folders */
  folders: Map<string, FolderTreeNode>;
  /** Root-level workflows (no path) */
  workflows: Workflow[];
}

/**
 * Parses a path string into segments, handling edge cases
 */
function parsePath(path: string | undefined): string[] {
  if (!path) return [];

  // Trim leading/trailing slashes and split
  const trimmed = path.replace(/^\/+|\/+$/g, '');
  if (!trimmed) return [];

  // Split and filter out empty segments (handles "a//b")
  return trimmed.split('/').filter(segment => segment.length > 0);
}

/**
 * Creates a new folder tree node
 */
function createFolderNode(name: string, fullPath: string): FolderTreeNode {
  return {
    name,
    fullPath,
    folders: new Map(),
    workflows: [],
    totalWorkflowCount: 0,
  };
}

/**
 * Recursively calculates the total workflow count for a folder
 */
function calculateTotalCount(node: FolderTreeNode): number {
  let count = node.workflows.length;
  for (const child of node.folders.values()) {
    count += calculateTotalCount(child);
  }
  node.totalWorkflowCount = count;
  return count;
}

/**
 * Builds a folder tree from a list of workflows
 */
export function buildFolderTree(workflows: Workflow[]): FolderTree {
  const tree: FolderTree = {
    folders: new Map(),
    workflows: [],
  };

  // Sort workflows by priority first
  const sortedWorkflows = [...workflows].sort((a, b) => (a.priority ?? 0) - (b.priority ?? 0));

  for (const workflow of sortedWorkflows) {
    const segments = parsePath(workflow.path);

    if (segments.length === 0) {
      // No path - add to root level
      tree.workflows.push(workflow);
    } else {
      // Navigate/create folder hierarchy
      let currentLevel = tree.folders;
      let currentPath = '';

      for (let i = 0; i < segments.length; i++) {
        const segment = segments[i];
        currentPath = currentPath ? `${currentPath}/${segment}` : segment;

        if (!currentLevel.has(segment)) {
          currentLevel.set(segment, createFolderNode(segment, currentPath));
        }

        const folder = currentLevel.get(segment)!;

        if (i === segments.length - 1) {
          // Last segment - add workflow here
          folder.workflows.push(workflow);
        } else {
          // Continue navigating
          currentLevel = folder.folders;
        }
      }
    }
  }

  // Calculate total counts for all folders
  for (const folder of tree.folders.values()) {
    calculateTotalCount(folder);
  }

  return tree;
}

/**
 * Gets the IDs of first-level folders for default expansion
 */
export function getFirstLevelFolderIds(tree: FolderTree): string[] {
  return Array.from(tree.folders.keys()).map(name => NODE_IDS.folder(name));
}

/**
 * Gets all parent folder IDs for a given path
 */
export function getParentFolderIds(path: string | undefined): string[] {
  const segments = parsePath(path);
  if (segments.length === 0) return [];

  const ids: string[] = [];
  let currentPath = '';

  for (const segment of segments) {
    currentPath = currentPath ? `${currentPath}/${segment}` : segment;
    ids.push(NODE_IDS.folder(currentPath));
  }

  return ids;
}
