/**
 * Epoca Host API — TypeScript type definitions.
 *
 * Available as `window.epoca` inside sandboxed SPA tabs.
 * All methods return Promises that resolve/reject via the host.
 *
 * Requires corresponding permissions in manifest.toml:
 *   sign/getAddress → [permissions] sign = true
 *   statements.*    → [permissions] statements = true
 *   chain.*         → [permissions] chain = true
 *   data.*          → [permissions] data = true
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

type EpocaEventMap = {
  statement: StatementEvent;
  dataConnected: DataConnectedEvent;
  dataMessage: DataMessageEvent;
  dataClosed: DataClosedEvent;
  dataError: DataErrorEvent;
};

interface Epoca {
  /** Sign a payload. The user sees a confirmation dialog. */
  sign(payload: string): Promise<string>;
  /** Get the app's derived public address. */
  getAddress(): Promise<string>;

  readonly statements: EpocaStatements;
  readonly data: EpocaData;
  readonly chain: EpocaChain;

  /** Subscribe to host-pushed events. */
  on<K extends keyof EpocaEventMap>(event: K, callback: (data: EpocaEventMap[K]) => void): void;
  /** Unsubscribe from an event. */
  off<K extends keyof EpocaEventMap>(event: K, callback: (data: EpocaEventMap[K]) => void): void;
}

declare global {
  interface Window {
    epoca: Epoca;
  }
}

export {};
