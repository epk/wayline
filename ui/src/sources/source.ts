import type { LatencyDoc, Settings, TelemetryDoc, UpdateStatus, WifiDoc } from '../types.ts';

export interface HistoryQuery {
  start: number;
  end: number;
  step: number;
}

export interface LatencyQuery extends HistoryQuery {
  target: string;
  tunnel?: string;
  all?: boolean;
  uplinks?: boolean;
  include_wan?: boolean;
  endpoints?: boolean;
}

export interface Revisioned {
  expected_revision: string;
}

export interface UpdateReply {
  started?: boolean;
  update?: UpdateStatus;
}

export interface DataSource {
  getState(): Promise<unknown>;
  getHistory(query: LatencyQuery): Promise<LatencyDoc>;
  getWifiHistory(query: HistoryQuery): Promise<WifiDoc>;
  getTelemetryHistory(query: HistoryQuery): Promise<TelemetryDoc>;
  getDiagnostics?(): Promise<unknown>;
  setConfig?(request: Revisioned & { config: Settings }): Promise<unknown>;
  selectTunnel?(request: Revisioned & { section_id: string }): Promise<unknown>;
  setTunnelEnabled?(
    request: Revisioned & { section_id: string; enabled: boolean },
  ): Promise<{ enabled?: boolean; monitor_pending?: boolean }>;
  purgeHistory?(request: { confirm: true }): Promise<unknown>;
  checkUpdate?(): Promise<UpdateReply>;
  installUpdate?(): Promise<UpdateReply>;
  clearUpdate?(): Promise<UpdateReply>;
}

export type WriteCommand =
  | 'setConfig'
  | 'selectTunnel'
  | 'setTunnelEnabled'
  | 'purgeHistory'
  | 'checkUpdate'
  | 'installUpdate'
  | 'clearUpdate';
