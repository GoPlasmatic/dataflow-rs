import { Folder } from 'lucide-react';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import type { FolderTreeNode } from '../utils/folderTree';
import { TreeNode } from './TreeNode';
import { WorkflowNode } from './WorkflowNode';
import { TREE_COLORS } from './colors';

interface FolderNodeProps {
  folder: FolderTreeNode;
  level: number;
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  expandedNodes: Set<string>;
  toggleNode: (id: string) => void;
  debugMode?: boolean;
}

export function FolderNode({
  folder,
  level,
  selection,
  onSelect,
  expandedNodes,
  toggleNode,
  debugMode = false,
}: FolderNodeProps) {
  const folderId = `folder-${folder.fullPath}`;
  const isExpanded = expandedNodes.has(folderId);
  const hasChildren = folder.folders.size > 0 || folder.workflows.length > 0;

  // Sort child folders alphabetically
  const sortedFolders = Array.from(folder.folders.values()).sort((a, b) =>
    a.name.localeCompare(b.name)
  );

  // Workflows are already sorted by priority from buildFolderTree

  return (
    <TreeNode
      label={`${folder.name} (${folder.totalWorkflowCount})`}
      icon={<Folder size={14} />}
      iconColor={TREE_COLORS.folder}
      isExpanded={isExpanded}
      hasChildren={hasChildren}
      level={level}
      onToggle={() => toggleNode(folderId)}
      onClick={() => toggleNode(folderId)}
    >
      {/* Render child folders first (alphabetically) */}
      {sortedFolders.map((childFolder) => (
        <FolderNode
          key={childFolder.fullPath}
          folder={childFolder}
          level={level + 1}
          selection={selection}
          onSelect={onSelect}
          expandedNodes={expandedNodes}
          toggleNode={toggleNode}
          debugMode={debugMode}
        />
      ))}

      {/* Render workflows in this folder (by priority) */}
      {folder.workflows.map((workflow) => (
        <WorkflowNode
          key={workflow.id}
          workflow={workflow}
          level={level + 1}
          selection={selection}
          onSelect={onSelect}
          expandedNodes={expandedNodes}
          toggleNode={toggleNode}
          debugMode={debugMode}
        />
      ))}
    </TreeNode>
  );
}
