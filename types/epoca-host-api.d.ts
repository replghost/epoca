/**
 * Epoca Host API — TypeScript type definitions.
 *
 * Available as `window.host` inside sandboxed SPA tabs.
 * All methods return Promises that resolve/reject via the host.
 *
 * Requires corresponding permissions in manifest.toml:
 *   sign/getAddress → [permissions] sign = true
 *   statements.*    → [permissions] statements = true
 *   chain.*         → [permissions] chain = true
 *   data.*          → [permissions] data = true
 *   media.*         → [permissions] media = ["camera", "audio"]
 */

interface EpocaStatements {
  /** Publish data to a channel. Other subscribers receive it as a 'statement' event. */
  write(channel: string, data: string): Promise<true>;
  /** Subscribe to a channel. Incoming messages arrive via epoca.on('statement', ...). */
  subscribe(channel: string): Promise<true>;
}

interface EpocaData {
  /** Request a P2P data connection to a peer address. Shows approval dialog. */
  connect(peerAddress: string): Promise<number>;
  /** Send data over an established connection. */
  send(connId: number, data: string): Promise<true>;
  /** Close a data connection. */
  close(connId: number): Promise<true>;
}

interface MediaConstraints {
  audio?: boolean;
  video?: boolean;
}

interface EpocaMedia {
  /**
   * Request access to camera/audio. Resolves immediately with a pending stream ID.
   * Track readiness arrives via the 'mediaTrackReady' push event.
   * Requires manifest.toml: media = ["camera"] and/or ["audio"].
   */
  getUserMedia(constraints: MediaConstraints): Promise<number>;
  /**
   * Open a peer-to-peer media session using previously captured track IDs.
   * Resolves immediately with a pending session ID.
   * Connection readiness arrives via the 'mediaConnected' push event.
   */
  connect(peer: string, trackIds?: number[]): Promise<number>;
  /** Close a media session. The host cleans up tracks and connections. */
  close(sessionId: number): Promise<true>;
  /** Attach a local or remote track to a DOM element by element ID. */
  attachTrack(trackId: number, elementId: string): Promise<true>;
}

interface EpocaChain {
  /** Query the chain via JSON-RPC (read-only methods only). */
  query(method: string, params?: unknown[]): Promise<unknown>;
  /** Submit an extrinsic to the chain. Shows approval dialog. */
  submit(callData: string): Promise<unknown>;
}

interface StatementEvent {
  author: string;
  channel: string;
  data: string;
  timestamp: number;
}

interface DataConnectedEvent {
  connId: number;
  peer: string;
}

interface DataMessageEvent {
  connId: number;
  data: string;
}

interface DataClosedEvent {
  connId: number;
  reason: string;
}

interface DataErrorEvent {
  connId: number;
  error: string;
}

interface MediaTrackReadyEvent {
  /** Stream ID returned by getUserMedia. */
  streamId: number;
  /** Individual track ID within the stream. */
  trackId: number;
  /** "audio" or "video". */
  kind: string;
}

interface MediaConnectedEvent {
  sessionId: number;
  peer: string;
}

interface MediaRemoteTrackEvent {
  sessionId: number;
  trackId: number;
  kind: string;
}

interface MediaClosedEvent {
  sessionId: number;
  reason: string;
}

interface MediaErrorEvent {
  sessionId: number;
  error: string;
}

type EpocaEventMap = {
  statement: StatementEvent;
  dataConnected: DataConnectedEvent;
  dataMessage: DataMessageEvent;
  dataClosed: DataClosedEvent;
  dataError: DataErrorEvent;
  mediaTrackReady: MediaTrackReadyEvent;
  mediaConnected: MediaConnectedEvent;
  mediaRemoteTrack: MediaRemoteTrackEvent;
  mediaClosed: MediaClosedEvent;
  mediaError: MediaErrorEvent;
};

interface Epoca {
  /** Sign a payload. The user sees a confirmation dialog. */
  sign(payload: string): Promise<string>;
  /** Get the app's derived public address. */
  getAddress(): Promise<string>;

  readonly statements: EpocaStatements;
  readonly data: EpocaData;
  readonly chain: EpocaChain;
  readonly media: EpocaMedia;

  /** Subscribe to host-pushed events. */
  on<K extends keyof EpocaEventMap>(event: K, callback: (data: EpocaEventMap[K]) => void): void;
  /** Unsubscribe from an event. */
  off<K extends keyof EpocaEventMap>(event: K, callback: (data: EpocaEventMap[K]) => void): void;
}

declare global {
  interface Window {
    host: Epoca;
  }
}

export {};
