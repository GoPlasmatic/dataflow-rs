import { useState, useMemo, useEffect, useRef } from 'react';
import { Layers } from 'lucide-react';
import type { Workflow } from '../../../types';
import type { TreeSelectionType } from '../WorkflowVisualizer';
import { useDebugger } from '../context';
import { TreeNode, WorkflowNode, TREE_COLORS } from '../components';

interface TreeViewProps {
  workflows: Workflow[];
  selection: TreeSelectionType;
  onSelect: (selection: TreeSelectionType) => void;
  /** Enable debug mode with state indicators */
  debugMode?: boolean;
}

export function TreeView({ workflows, selection, onSelect, debugMode = false }: TreeViewProps) {
  // Always call hook unconditionally (React rules of hooks)
  const debuggerContext = useDebugger();
  const effectiveDebugContext = debugMode ? debuggerContext : null;

  const [expandedNodes, setExpandedNodes] = useState<Set<string>>(() => {
    // Initially expand the root "Workflows" node and first workflow
    const initial = new Set(['workflows-root']);
    if (workflows.length > 0) {
      initial.add(`workflow-${workflows[0].id}`);
    }
    return initial;
  });

  const sortedWorkflows = useMemo(() => {
    return [...workflows].sort((a, b) => (a.priority ?? 0) - (b.priority ?? 0));
  }, [workflows]);

  // Track last selected step to prevent redundant selections
  const lastSelectedRef = useRef<{ workflowId: string; taskId?: string } | null>(null);

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

    // Auto-expand nodes to show current step
    setExpandedNodes((prev) => {
      const next = new Set(prev);
      next.add('workflows-root');
      next.add(`workflow-${workflow_id}`);
      next.add(`tasks-${workflow_id}`);
      if (task_id) {
        next.add(`task-${workflow_id}-${task_id}`);
      }
      return next;
    });

    // Auto-select the current task or workflow
    if (task_id) {
      const workflow = workflows.find(w => w.id === workflow_id);
      const task = workflow?.tasks.find(t => t.id === task_id);
      if (workflow && task) {
        lastSelectedRef.current = { workflowId: workflow_id, taskId: task_id };
        onSelect({ type: 'task', task, workflow });
      }
    } else {
      lastSelectedRef.current = { workflowId: workflow_id };
    }
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

  return (
    <div className={`df-tree-view ${debugMode ? 'df-tree-view-debug' : ''}`}>
      <TreeNode
        label="Workflows"
        icon={<Layers size={14} />}
        iconColor={TREE_COLORS.workflow}
        isExpanded={isRootExpanded}
        hasChildren={sortedWorkflows.length > 0}
        level={0}
        onToggle={() => toggleNode('workflows-root')}
        onClick={() => toggleNode('workflows-root')}
      >
        {sortedWorkflows.map((workflow) => (
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
