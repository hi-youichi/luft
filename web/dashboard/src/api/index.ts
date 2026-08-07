// ============================================================================
// Luft Dashboard API — Public Surface
// ============================================================================

// Core transport
export { LuftWsClient } from './ws-client.js';
export type {
  WsClientConfig,
  ReconnectOptions,
} from './ws-client.js';

// MCP client
export { McpClient } from './mcp-client.js';
export type { McpClientConfig } from './mcp-client.js';

// Run session client
export { RunClient, createRunClient } from './run-client.js';
export type {
  StartOptions,
  RunSession,
  RunCompleteResult,
} from './run-client.js';

// Unified facade
export { LuftClient } from './client.js';
export type { LuftClientConfig } from './client.js';

// All protocol types and errors
export type {
  ConnectionState,
  TokenUsage,
  RunStatus,
  LogLevel,
  AgentStatus,
  ProgressDelta,
  PlanPhase,
  FindingLocation,
  Finding,
  RunStartedEvent,
  PhaseStartedEvent,
  AgentStartedEvent,
  AgentProgressEvent,
  AcpRawEvent,
  AcpRequestEvent,
  AgentDoneEvent,
  PhaseDoneEvent,
  RunDoneEvent,
  LogEvent,
  BudgetSetEvent,
  ReportEmittedEvent,
  ParallelStartedEvent,
  ParallelDoneEvent,
  WorkflowStartedEvent,
  WorkflowDoneEvent,
  ConvergeStartedEvent,
  ConvergeDoneEvent,
  PipelineStartedEvent,
  PipelineStageStartedEvent,
  PipelineItemDoneEvent,
  PipelineDoneEvent,
  SchemaRetryEvent,
  PlanPreviewEvent,
  SignalReceivedEvent,
  AgentEvent,
  RunClientMessage,
  RunServerMessage,
  ExecuteWorkflowRequest,
  ExecuteWorkflowResponse,
  WorkflowFile,
  ListRunsRequest,
  RunSummary,
  ListRunsResponse,
  GetRunEventsRequest,
  RunEventsResponse,
  CancelRunResponse,
  PhaseAgentView,
  PhaseView,
  RunStatusResponse,
  JsonRpcRequest,
  JsonRpcNotification,
  JsonRpcResponse,
  JsonRpcError,
  ServerInfo,
  McpTool,
  CallToolResult,
  McpResource,
  ReadResourceResult,
} from './types.js';

export {
  LuftWsError,
  McpError,
  isRunServerMessage,
  isJsonRpcResponse,
} from './types.js';
