import { AlertTriangle, CheckCircle, Layers, Box, FileJson } from 'lucide-react';
import { countLeafSteps } from '../../types';
import type { Workflow } from '../../types';

interface StatusBarProps {
  workflows: Workflow[];
  workflowsError: string | null;
  messageError: string | null;
  cursorPosition?: { line: number; column: number };
}

export function StatusBar({
  workflows,
  workflowsError,
  messageError,
  cursorPosition,
}: StatusBarProps) {
  const hasErrors = workflowsError || messageError;
  const errorCount = (workflowsError ? 1 : 0) + (messageError ? 1 : 0);
  // `tasks` holds *steps*, so a group counts as one element regardless of how
  // many tasks it contains. Descend before counting.
  const taskCount = workflows.reduce((sum, w) => sum + countLeafSteps(w.tasks ?? []), 0);

  return (
    <footer className="status-bar">
      <div className="status-bar-left">
        {/* Error/Success indicator */}
        <div className={`status-item ${hasErrors ? 'status-error' : 'status-success'}`}>
          {hasErrors ? (
            <>
              <AlertTriangle size={14} />
              <span>{errorCount} error{errorCount > 1 ? 's' : ''}</span>
            </>
          ) : (
            <>
              <CheckCircle size={14} />
              <span>No errors</span>
            </>
          )}
        </div>

        <div className="status-divider" />

        {/* Workflow count */}
        <div className="status-item">
          <Layers size={14} />
          <span>Workflows: {workflows.length}</span>
        </div>

        {/* Task count */}
        <div className="status-item">
          <Box size={14} />
          <span>Tasks: {taskCount}</span>
        </div>
      </div>

      <div className="status-bar-right">
        {/* Cursor position */}
        {cursorPosition && (
          <div className="status-item">
            <span>Ln {cursorPosition.line}, Col {cursorPosition.column}</span>
          </div>
        )}

        <div className="status-divider" />

        {/* File type */}
        <div className="status-item">
          <FileJson size={14} />
          <span>JSON</span>
        </div>
      </div>
    </footer>
  );
}
