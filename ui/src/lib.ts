// Main component
export { WorkflowVisualizer } from './components/workflow-visualizer';
export type {
  WorkflowVisualizerProps,
  TreeSelectionType,
} from './components/workflow-visualizer';

// Sub-components (for advanced usage)
export {
  WorkflowCard,
  TaskRow,
  FunctionTypeBadge,
  ConditionBadge,
} from './components/workflow-visualizer/cards';

export { RulesListView, WorkflowFlowView, TreeView } from './components/workflow-visualizer/views';

// Debug components
export {
  DebuggerControls,
  MessageInputPanel,
  MessageStatePanel,
  DebugInfoBubble,
  DebugStateBadge,
  IntegratedDebugToolbar,
} from './components/workflow-visualizer/debug';

// Context and hooks
export {
  ThemeProvider,
  useTheme,
  DebuggerProvider,
  useDebugger,
  useDebuggerOptional,
} from './components/workflow-visualizer/context';
export type { Theme } from './components/workflow-visualizer/context';

// Debug hooks
export {
  useTreeNodeDebugState,
  useWorkflowDebugState,
  useWorkflowConditionDebugState,
  useTaskDebugState,
  useTaskConditionDebugState,
} from './components/workflow-visualizer/hooks';
export type { TreeNodeDebugState } from './components/workflow-visualizer/hooks';

// Common components
export { SearchInput, JsonViewer, ErrorBoundary } from './components/common';

// Types
export type {
  Workflow,
  Task,
  FunctionConfig,
  JsonLogicValue,
  MapMapping,
  MappingItem,
  MapFunctionInput,
  ValidationRule,
  ValidationFunctionInput,
  BuiltinFunctionType,
  // Debug types
  Message,
  ErrorInfo,
  Change,
  AuditTrail,
  DebugNodeState,
  ConditionResult,
  ExecutionStep,
  ExecutionTrace,
  StepResult,
  PlaybackState,
  DebuggerState,
  DebuggerAction,
  // Engine types
  DataflowEngine,
  EngineFactory,
  // Debug config type
  DebugConfig,
} from './types';

export {
  isBuiltinFunction,
  getFunctionDisplayInfo,
  // Debug helpers
  createEmptyMessage,
  cloneMessage,
  getMessageAtStep,
  getChangesAtStep,
  getWorkflowState,
  getTaskState,
} from './types';

// Engine adapters
export { WasmEngineAdapter, defaultEngineFactory } from './engines';

// Styles
import './components/workflow-visualizer/styles/index.css';
// React Flow styles (required for diagram visualizer)
import '@xyflow/react/dist/style.css';
// DataLogic UI styles (for diagram visualizer)
import '@goplasmatic/datalogic-ui/styles.css';
