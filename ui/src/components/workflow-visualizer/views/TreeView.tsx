import { useState, useMemo, useEffect, useRef } from 'react';
import { Layers } from 'lucide-react';
import type { Workflow } from '../../../types';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import { useDebuggerOptional } from '../context';
import { TreeNode, WorkflowNode, FolderNode, TREE_COLORS } from '../components';
import { buildFolderTree, getFirstLevelFolderIds, getParentFolderIds } from '../utils/folderTree';

interface TreeViewProps {
  workflows: Workflow[];
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  /** Enable debug mode with state indicators */
  debugMode?: boolean;
}

export function TreeView({ workflows, selection, onSelect, debugMode = false }: TreeViewProps) {
  // Use optional hook that returns null if no provider exists
  const debuggerContext = useDebuggerOptional();
  const effectiveDebugContext = debugMode ? debuggerContext : null;

  // Build folder tree from workflows
  const folderTree = useMemo(() => buildFolderTree(workflows), [workflows]);

  // Sort root-level folders alphabetically
  const sortedRootFolders = useMemo(() => {
    return Array.from(folderTree.folders.values()).sort((a, b) =>
      a.name.localeCompare(b.name)
    );
  }, [folderTree]);

  // Root-level workflows (no path) - already sorted by priority in buildFolderTree
  const rootWorkflows = folderTree.workflows;

  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(() => {
    // Initially expand the root "Workflows" node and first-level folders
    const initial = new Set(['workflows-root']);
    getFirstLevelFolderIds(folderTree).forEach(id => initial.add(id));
    return initial;
  });

  // Update expanded nodes when folder tree changes (e.g., new workflows loaded)
  useEffect(() => {
    setExpandedNodes((prev) => {
      const next = new Set(prev);
      next.add('workflows-root');
      // Expand first-level folders by default
      getFirstLevelFolderIds(folderTree).forEach(id => next.add(id));
      return next;
    });
  }, [folderTree]);

  // Expand the first workflow when workflows change
  useEffect(() => {
    const allWorkflows = [...folderTree.workflows];
    // Also collect workflows from folders
    function collectWorkflows(folders: Map<string, typeof folderTree.folders extends Map<string, infer T> ? T : never>) {
      for (const folder of folders.values()) {
        allWorkflows.push(...folder.workflows);
        collectWorkflows(folder.folders);
      }
    }
    collectWorkflows(folderTree.folders);

    if (allWorkflows.length > 0) {
      // Sort by priority and expand the first one
      allWorkflows.sort((a, b) => (a.priority ?? 0) - (b.priority ?? 0));
      setExpandedNodes((prev) => {
        const next = new Set(prev);
        next.add(`workflow-${allWorkflows[0].id}`);
        // Also expand parent folders if needed
        getParentFolderIds(allWorkflows[0].path).forEach(id => next.add(id));
        return next;
      });
    }
  }, [folderTree]);

  // Track last selected step to prevent redundant selections
  const lastSelectedRef = useRef<{ workflowId: string; taskId?: string } | null>(null);

  // Ref for tree container (used for auto-scroll)
  const treeContainerRef = useRef<HTMLDivElement>(null);

  // Auto-expand and select based on current debug step
  useEffect(() => {
    // Don't auto-select if at step -1 (ready state) or no step
    if (!debugMode || !effectiveDebugContext?.currentStep ||
        effectiveDebugContext.state.currentStepIndex < 0) {
      return;
    }

    const { workflow_id, task_id } = effectiveDebugContext.currentStep;

    // Check if we already selected this step
    if (lastSelectedRef.current?.workflowId === workflow_id &&
        lastSelectedRef.current?.taskId === task_id) {
      return;
    }

    // Find the workflow to get its path
    const workflow = workflows.find(w => w.id === workflow_id);

    // Auto-expand nodes to show current step
    setExpandedNodes((prev) => {
      const next = new Set(prev);
      next.add('workflows-root');
      // Expand parent folders if workflow has a path
      if (workflow?.path) {
        getParentFolderIds(workflow.path).forEach(id => next.add(id));
      }
      next.add(`workflow-${workflow_id}`);
      next.add(`tasks-${workflow_id}`);
      if (task_id) {
        next.add(`task-${workflow_id}-${task_id}`);
      }
      return next;
    });

    // Auto-select the current task or workflow
    if (task_id) {
      const task = workflow?.tasks.find(t => t.id === task_id);
      if (workflow && task) {
        lastSelectedRef.current = { workflowId: workflow_id, taskId: task_id };
        onSelect({ type: 'task', task, workflow });
      }
    } else {
      lastSelectedRef.current = { workflowId: workflow_id };
    }

    // Auto-scroll to current step after a short delay (to allow DOM to update)
    setTimeout(() => {
      const currentStepElement = treeContainerRef.current?.querySelector('[data-current-step="true"]');
      if (currentStepElement) {
        currentStepElement.scrollIntoView({
          behavior: 'smooth',
          block: 'nearest',
        });
      }
    }, 50);
  }, [debugMode, effectiveDebugContext?.currentStep, effectiveDebugContext?.state.currentStepIndex, workflows, onSelect]);

  const toggleNode = (id: string) => {
    setExpandedNodes((prev) => {
      const next = new Set(prev);
      if (next.has(id)) {
        next.delete(id);
      } else {
        next.add(id);
      }
      return next;
    });
  };

  const isRootExpanded = expandedNodes.has('workflows-root');
  const totalWorkflowCount = workflows.length;

  return (
    <div ref={treeContainerRef} className={`df-tree-view ${debugMode ? 'df-tree-view-debug' : ''}`}>
      <TreeNode
        label="Workflows"
        icon={<Layers size={14} />}
        iconColor={TREE_COLORS.workflow}
        isExpanded={isRootExpanded}
        hasChildren={totalWorkflowCount > 0}
        level={0}
        onToggle={() => toggleNode('workflows-root')}
        onClick={() => toggleNode('workflows-root')}
      >
        {/* Render folders first (alphabetically) */}
        {sortedRootFolders.map((folder) => (
          <FolderNode
            key={folder.fullPath}
            folder={folder}
            level={1}
            selection={selection}
            onSelect={onSelect}
            expandedNodes={expandedNodes}
            toggleNode={toggleNode}
            debugMode={debugMode}
          />
        ))}

        {/* Render root-level workflows (by priority) */}
        {rootWorkflows.map((workflow) => (
          <WorkflowNode
            key={workflow.id}
            workflow={workflow}
            level={1}
            selection={selection}
            onSelect={onSelect}
            expandedNodes={expandedNodes}
            toggleNode={toggleNode}
            debugMode={debugMode}
          />
        ))}
      </TreeNode>
    </div>
  );
}
