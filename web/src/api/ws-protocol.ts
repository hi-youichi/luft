import type { AgentEvent, RunId } from './types'

export type WSMessageType =
  | 'event'
  | 'subscribe'
  | 'unsubscribe'
  | 'subscribed'
  | 'unsubscribed'
  | 'ping'
  | 'pong'
  | 'error'
  | 'close'

export interface WSEnvelope {
  type: WSMessageType
  ts: string
  [key: string]: unknown
}

export interface WSSubscribeMessage {
  type: 'subscribe'
  topic: string
  ts?: string
}

export interface WSUnsubscribeMessage {
  type: 'unsubscribe'
  topic: string
  ts?: string
}

export interface WSPingMessage {
  type: 'ping'
  ts?: string
}

export interface WSSubscribedMessage {
  type: 'subscribed'
  topic: string
  ts: string
}

export interface WSUnsubscribedMessage {
  type: 'unsubscribed'
  topic: string
  ts: string
}

export interface WSPongMessage {
  type: 'pong'
  ts: string
}

export interface WSErrorMessage {
  type: 'error'
  code: string
  message: string
  ts: string
}

export interface WSCloseMessage {
  type: 'close'
  code: number
  reason: string
  ts: string
}

export type WSClientMessage =
  | WSSubscribeMessage
  | WSUnsubscribeMessage
  | WSPingMessage

export type WSServerMessage =
  | { type: 'event'; event: AgentEvent; topic?: string; ts: string }
  | WSSubscribedMessage
  | WSUnsubscribedMessage
  | WSPongMessage
  | WSErrorMessage
  | WSCloseMessage

export type WSMessage = WSClientMessage | WSServerMessage

export function isServerMessage(msg: WSMessage): msg is WSServerMessage {
  return ['event', 'subscribed', 'unsubscribed', 'pong', 'error', 'close'].includes(msg.type)
}

export function isClientMessage(msg: WSMessage): msg is WSClientMessage {
  return ['subscribe', 'unsubscribe', 'ping'].includes(msg.type)
}

export function isAgentEventMessage(msg: WSMessage): msg is { type: 'event'; event: AgentEvent; topic?: string; ts: string } {
  return msg.type === 'event'
}

export function isErrorMessage(msg: WSMessage): msg is WSErrorMessage {
  return msg.type === 'error'
}

export type WSTopic = `runs:${RunId}` | `runs:${RunId}:events` | 'runs:all' | 'system'

export function runTopic(runId: RunId): WSTopic {
  return `runs:${runId}`
}

export function runEventsTopic(runId: RunId): WSTopic {
  return `runs:${runId}:events`
}

export const SYSTEM_TOPIC: WSTopic = 'system'
export const ALL_RUNS_TOPIC: WSTopic = 'runs:all'

export interface WSSubscription {
  topic: WSTopic
  active: boolean
  subscribed_at?: number
}

export function createSubscribeMessage(topic: WSTopic): WSSubscribeMessage {
  return { type: 'subscribe', topic }
}

export function createUnsubscribeMessage(topic: WSTopic): WSUnsubscribeMessage {
  return { type: 'unsubscribe', topic }
}

export function createPingMessage(): WSPingMessage {
  return { type: 'ping' }
}
