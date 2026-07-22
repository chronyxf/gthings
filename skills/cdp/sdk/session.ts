/**
 * CDP Session: persistent WebSocket to Chrome's browser endpoint.
 * Auto-injects sessionId for the active target on every call.
 *
 * Connect with `flatten: true` so all sessions share one WS (no nested
 * Target.sendMessageToTarget envelopes).
 *
 * Detection priority:
 *  1. DevToolsActivePort file scan (Google Chrome, Dia)
 *  2. HTTP check on port 9222 (custom browser instances)
 *
 * Dia auto-allow: on macOS, if the WS-open stalls past autoAllowDelayMs and
 * the browser is Dia, the SDK fires a Return keystroke (osascript) to dismiss
 * Dia's "Allow debugging connection?" prompt — no manual click needed.
 * Requires macOS Accessibility permission; otherwise falls back to timeoutMs.
 *
 * Version tracking: static Session.version exposes SDK version. After connect(),
 * browserVersion gives the detected browser version. isStale() checks whether
 * the daemon/binary version is stale relative to the SDK version at connect time.
 */

import { execFile } from 'child_process';
import { bindDomains, type Domains, type Transport } from './generated.ts';

const VERSION = '0.8.0';

type Pending = {
  resolve: (v: unknown) => void;
  reject: (e: unknown) => void;
};

export type ConnectOptions = {
  /** Full WS URL: ws://host:port/devtools/browser/<id>. */
  wsUrl?: string;
  /** Read DevToolsActivePort from profile dir. Polls up to 30s. */
  profileDir?: string;
  /** Connect by debug port. Fetches /json/version to get WS URL. */
  port?: number;
  /** Per-candidate WS-open timeout in ms. Default 5000. */
  timeoutMs?: number;
  /** Opt OUT of auto-dismissing Dia's "Allow debugging connection?" prompt.
   *  On by default (macOS, Dia only): when the WS-open stalls past
   *  autoAllowDelayMs, the SDK fires a Return at the Dia process via osascript,
   *  so connect needs no manual click — a no-op for every other browser.
   *  Set false to disable. Requires macOS Accessibility permission. */
  autoAllow?: boolean;
  /** ms after WS creation before auto-dismissing Dia's prompt. Default 600. */
  autoAllowDelayMs?: number;
  /** Override max agent tabs for this session. Default Infinity (no limit). */
  maxAgentTabs?: number;
};

export type DetectedBrowser = {
  name: string;
  profileDir: string;
  port: number;
  wsPath: string;
  wsUrl: string;
  mtimeMs: number;
  /** Browser version string from /json/version (only available via HTTP detection). */
  version?: string;
};

export class Session implements Transport {
  /** Current SDK version string. */
  static get version(): string { return VERSION; }

  private ws?: WebSocket;
  private nextId = 1;
  private pending = new Map<number, Pending>();
  private activeSessionId: string | undefined;
  static MAX_PENDING = 1000;
  static MAX_AGENT_TABS = Infinity;
  private eventListeners: Array<(method: string, params: unknown, sessionId?: string) => void> = [];

  /** Agent-tab registry: tracks targetIds created via createTarget(). */
  private agentTabs = new Map<string, string | undefined>();
  /** Max agent tabs for this instance (defaults to static MAX_AGENT_TABS — Infinity = no limit). */
  private maxAgentTabs: number;

  /** Serialized close queue — each closeTab waits for the previous one. */
  private closeQueue: Promise<void> = Promise.resolve();

  /** Pre-call observer, invoked before each CDP call if recordCalls is true. */
  private callObserver?: (method: string, params: unknown) => void;
  /** When true and a callObserver is set, invoke it before every CDP call. */
  private recordCalls = false;

  /**
   * Reconnect guard. Set to true while a reconnect is in-flight.
   * Prevents infinite reconnect loops: if the WS is still dead
   * after one reconnect attempt, give up and surface the error.
   */
  private _reconnecting = false;

  /**
   * Dedup for concurrent connect() calls. When multiple _call()
   * calls detect a dead WS simultaneously, this promise ensures
   * only one _connect() runs and all others wait for the same
   * result — no racing, no orphan WS connections.
   */
  private connectPromise?: Promise<void>;

  /** On by default: connect() auto-dismisses Dia's "Allow debugging connection?"
   *  prompt (macOS, via osascript Return) — a no-op for every other browser.
   *  Persisted so reconnects inherit the setting. */
  autoAllow = true;

  domains!: Domains;

  /** Browser version string detected at connect time (e.g. "Chrome/131.0.6778.109"). */
  private _browserVersion?: string;
  /** SDK version at the time connect() succeeded. Used by isStale(). */
  private _connectedAtVersion?: string;

  constructor(opts?: { autoAllow?: boolean; maxAgentTabs?: number; recordCalls?: boolean }) {
    if (opts?.autoAllow !== undefined) this.autoAllow = opts.autoAllow;
    if (opts?.recordCalls !== undefined) this.recordCalls = opts.recordCalls;
    this.maxAgentTabs = opts?.maxAgentTabs ?? Session.MAX_AGENT_TABS;
    this.domains = bindDomains(this);
    for (const k of Object.keys(this.domains) as (keyof Domains)[]) {
      (this as any)[k] = this.domains[k];
    }
  }

  /** The browser version detected at connect time, if available. */
  get browserVersion(): string | undefined { return this._browserVersion; }

  /**
   * Check if the connected daemon/browser is stale relative to this SDK version.
   * Returns true when the SDK version has changed since connect() was called,
   * indicating the browser process should be restarted for compatibility.
   */
  isStale(): boolean {
    return this._connectedAtVersion !== undefined && this._connectedAtVersion !== VERSION;
  }

  /**
   * Connect to a CDP-enabled browser.
   *
   * Auto-detects running browser instances if no opts are given.
   * Dedups concurrent calls via connectPromise — if a reconnect
   * is already in flight, new callers ride on it instead of
   * triggering a second one.
   */
  async connect(opts: ConnectOptions = {}): Promise<void> {
    if (this.isConnected()) return;
    // Dedup: another reconnect is already in-flight — wait for it
    if (this.connectPromise) {
      await this.connectPromise;
      return;
    }

    this.connectPromise = this._connect(opts);
    try {
      await this.connectPromise;
    } catch (e) {
      this.connectPromise = undefined;
      throw e;
    }
    this.connectPromise = undefined;
  }

  /**
   * Internal connect — the actual connection logic.
   * Extracted so connect() can dedup callers via connectPromise.
   */
  private async _connect(opts: ConnectOptions = {}): Promise<void> {
    const timeoutMs = opts.timeoutMs ?? 5_000;
    const autoAllowDelayMs = opts.autoAllowDelayMs ?? 600;
    if (opts.autoAllow !== undefined) this.autoAllow = opts.autoAllow;
    if (opts.maxAgentTabs !== undefined) this.maxAgentTabs = opts.maxAgentTabs;

    if (opts.port) {
      const wsUrl = await discoverByPort(opts.port);
      const name = this.autoAllow ? await browserNameFor(opts, wsUrl) : undefined;
      await this.openWs(wsUrl, timeoutMs, { autoAllow: this.autoAllow, name, autoAllowDelayMs });
      this._connectedAtVersion = VERSION;
      return;
    }
    if (opts.wsUrl || opts.profileDir) {
      const wsUrl = await resolveWsUrl(opts);
      const name = this.autoAllow ? await browserNameFor(opts, wsUrl) : undefined;
      await this.openWs(wsUrl, timeoutMs, { autoAllow: this.autoAllow, name, autoAllowDelayMs });
      this._connectedAtVersion = VERSION;
      return;
    }

    const browsers = await detectBrowsers();
    if (browsers.length === 0) {
      throw new Error(
        `No running browser with remote debugging detected.\n\n` +
        `How to enable:\n` +
        `  Chrome:  open chrome://inspect/#remote-debugging → toggle ON\n` +
        `  Dia:     dia://inspect/#remote-debugging → toggle ON\n` +
        `  Terminal: <browser> --remote-debugging-port=9222\n\n` +
        `Or pass options explicitly:\n` +
        `  session.connect({ port: 9222 })\n` +
        `  session.connect({ wsUrl: "ws://127.0.0.1:9222/devtools/browser/<id>" })\n` +
        `  session.connect({ profileDir: "/path/to/profile" })`
      );
    }
    const errors: string[] = [];
    for (const b of browsers) {
      try {
        await this.openWs(b.wsUrl, timeoutMs, { autoAllow: this.autoAllow, name: b.name, autoAllowDelayMs });
        // Capture browser version from the detected browser entry (populated via HTTP /json/version)
        if (b.version) this._browserVersion = b.version;
        this._connectedAtVersion = VERSION;
        return;
      } catch (e) {
        errors.push(`  ${b.name} @ ${b.wsUrl}: ${e instanceof Error ? e.message : e}`);
      }
    }
    throw new Error(
      `No browser accepted connection.\n` +
      `If one of these is yours, click "Allow" then retry, or pass { timeoutMs: 30000 }:\n` +
      `${errors.join('\n')}`,
    );
  }

  private openWs(
    wsUrl: string,
    timeoutMs: number,
    allow?: { autoAllow: boolean; name?: string; autoAllowDelayMs: number },
  ): Promise<void> {
    return new Promise<void>((res, rej) => {
      const ws = new WebSocket(wsUrl);
      let done = false;
      let allowTried = false;
      const finish = (err?: Error) => {
        if (done) return;
        done = true;
        clearTimeout(timer);
        if (allowTimer) clearTimeout(allowTimer);
        if (err) { try { ws.close(); } catch { /* ignore */ } rej(err); }
        else res();
      };
      const timer = setTimeout(() => finish(new Error(`timed out after ${timeoutMs}ms`)), timeoutMs);
      // Dia auto-allow: if WS still CONNECTING after autoAllowDelayMs, dismiss prompt
      const allowTimer =
        allow && allow.autoAllow && allow.name === 'Dia' && process.platform === 'darwin'
          ? setTimeout(() => {
              if (done || allowTried) return;
              if (ws.readyState !== WebSocket.CONNECTING) return;
              allowTried = true;
              dismissDiaAllowPrompt();
            }, allow.autoAllowDelayMs)
          : null;
      ws.addEventListener('open', () => finish());
      ws.addEventListener('error', (e) => finish(new Error(`WS error: ${(e as any)?.message ?? 'connect failed (403 or port closed)'}`)));
      ws.addEventListener('message', (e) => this.onMessage(String(e.data)));
      ws.addEventListener('close', () => {
        // Only clear pending if this is the active WS — a parallel connect()
        // could create a phantom WS whose close handler would otherwise nuke
        // pending entries belonging to the current WS.
        if (this.ws === ws) {
          for (const [, p] of this.pending) p.reject(new Error('CDP socket closed'));
          this.pending.clear();
        }
        finish(new Error('WS closed before open (403 or port closed)'));
      });
      this.ws = ws;
    });
  }

  /** True when the CDP WebSocket is open and ready. */
  isConnected(): boolean { return this.ws?.readyState === WebSocket.OPEN; }

  /**
   * Disconnect from the browser and clean up agent tabs.
   * After calling close(), call connect() to reconnect.
   */
  close(): void {
    // Reset reconnect state so a subsequent connect() starts fresh.
    this._reconnecting = false;
    this.connectPromise = undefined;
    this.cleanupAgentTabs().catch(() => {});
    this.agentTabs.clear();
    this.closeQueue = Promise.resolve();
    this.ws?.close();
  }

  // ── Agent-tab registry ──────────────────────────────────────────────

  /**
   * Register a targetId as an agent-managed tab.
   * Typically called by createTarget() automatically.
   */
  registerAgentTab(targetId: string, sessionId?: string): void {
    this.agentTabs.set(targetId, sessionId);
  }

  /**
   * Remove a targetId from the agent-tab registry.
   * Typically called by closeTab() automatically.
   */
  unregisterAgentTab(targetId: string): void {
    this.agentTabs.delete(targetId);
  }

  /**
   * Check whether a targetId is registered as an agent tab.
   */
  isAgentTab(targetId: string): boolean {
    return this.agentTabs.has(targetId);
  }

  // ── Tab lifecycle ───────────────────────────────────────────────────

  /**
   * Create a new tab (agent-managed).
   *
   * Registers the resulting targetId in the agent-tab registry.
   * The tab opens in the default browser context, sharing cookies/session with the user.
   *
   * Use `session.use(targetId)` after creation to attach and get a sessionId.
   * Requires an active connection — call connect() first.
   */
  async createTarget(params: {
    url: string;
    width?: number;
    height?: number;
    background?: boolean;
    newWindow?: boolean;
    [key: string]: unknown;
  }): Promise<{ targetId: string; sessionId: string }> {
    const p = { ...params };
    if (p.width || p.height) {
      p.width = p.width || 1280;
      p.height = p.height || 800;
    }
    const result = await this.domains.Target.createTarget(p);
    const attach = await this.domains.Target.attachToTarget({
      targetId: result.targetId,
      flatten: true,
    });
    this.registerAgentTab(result.targetId, attach.sessionId);
    return { targetId: result.targetId, sessionId: attach.sessionId };
  }

  /**
   * Close a tab by targetId.
   *
   * Uses `window.close()` via Runtime.evaluate first (works on tabs opened
   * by script, which includes all tabs created via createTarget), then
   * `Target.closeTarget` to tear down the CDP session.
   *
   * Why two steps: `Target.closeTarget` alone succeeds in CDP but some
   * Some browsers (Dia) don't actually close the tab in the browser
   * window — the tab strip stays out of sync. `window.close()` triggers the
   * browser's own tab-close path, which reliably removes the tab. The short
   * delay gives the browser time to process the close before CDP teardown.
   *
   * Close operations are serialized so that the window.close() → delay →
   * Target.closeTarget sequence for one tab completes before the next begins.
   * Without serialization, interleaved closes can kill a session before
   * window.close() takes effect in the browser.
   *
   * Requires an active connection — call connect() first.
   *
   * @param targetId - The target to close.
   * @param sessionId - Optional sessionId for Runtime.evaluate.
   *                    If omitted and target is active, the active session is used.
   * @param force - Skip the agent-tab registry check. Default false.
   */
  async closeTab(targetId: string, sessionId?: string, force?: boolean): Promise<void> {
    if (!force && !this.agentTabs.has(targetId)) {
      throw new Error(
        `Target ${targetId} is not an agent tab. ` +
        `Use force:true to close anyway, or register it via registerAgentTab().`,
      );
    }
    // If sessionId not provided, look it up from the registry
    const effectiveSessionId = sessionId ?? this.agentTabs.get(targetId);
    this.unregisterAgentTab(targetId);
    const doClose = async () => {
      if (effectiveSessionId) {
        try {
          await this._call('Runtime.evaluate', { expression: 'window.close()' }, { sessionId: effectiveSessionId });
        } catch { /* session may already be detaching */ }
        await Bun.sleep(100);
      }
      try {
        await this.domains.Target.closeTarget({ targetId });
      } catch { /* already gone */ }
    };
    // Serialize: each close waits for the previous one to finish.
    this.closeQueue = this.closeQueue.then(doClose, doClose);
    return this.closeQueue;
  }

  /**
   * Close all registered agent tabs.
   * Each tab is closed via closeTab() with force=true. Errors are silently
   * swallowed. The registry is cleared before closing to prevent double-close.
   */
  async cleanupAgentTabs(): Promise<void> {
    const tabs = [...this.agentTabs.entries()];
    this.agentTabs.clear();
    for (const [targetId, sessionId] of tabs) {
      try {
        await this.closeTab(targetId, sessionId, true);
      } catch { /* best-effort cleanup */ }
    }
  }

  // ── Session management ──────────────────────────────────────────────

  /**
   * Pick a target and make subsequent calls auto-route to it.
   * Uses Target.attachToTarget with flatten:true (single-WS, sessionId-on-message).
   * Requires an active connection — call connect() first.
   */
  async use(targetId: string): Promise<string> {
    const r = await this._call('Target.attachToTarget', { targetId, flatten: true }) as { sessionId: string };
    this.activeSessionId = r.sessionId;
    return r.sessionId;
  }

  /** Set the active sessionId directly (e.g. one you already attached). */
  setActiveSession(sessionId: string | undefined): void { this.activeSessionId = sessionId; }
  /** Get the currently active sessionId. */
  getActiveSession(): string | undefined { return this.activeSessionId; }

  /** Get the number of pending CDP commands. */
  getPendingCount(): number { return this.pending.size; }

  // ── Call observer ───────────────────────────────────────────────────

  /**
   * Set a pre-call observer that fires before every CDP call.
   * Only invoked when recordCalls is true. Observer failures never break the
   * protocol call.
   */
  setCallObserver(callback: (method: string, params: unknown) => void): void {
    this.callObserver = callback;
  }

  /** Remove the call observer. */
  clearCallObserver(): void {
    this.callObserver = undefined;
  }

  // ── Events ──────────────────────────────────────────────────────────

  /**
   * Subscribe to all CDP events.
   * @returns A function that removes this listener when called.
   */
  onEvent(fn: (method: string, params: unknown, sessionId?: string) => void): () => void {
    this.eventListeners.push(fn);
    return () => { this.eventListeners = this.eventListeners.filter(x => x !== fn); };
  }

  /**
   * Wait for the next event matching `method` (and optional predicate).
   * If `targetId` is given, attaches to that target and filters by its
   * sessionId — critical for avoiding cross-fire in parallel tab use.
   */
  async waitFor<T = unknown>(method: string, predicate?: (params: T) => boolean, timeoutMs = 30_000, targetId?: string): Promise<T> {
    let targetSessionId: string | undefined;
    if (targetId !== undefined) {
      const prevSessionId = this.activeSessionId;
      targetSessionId = await this.use(targetId);
      this.activeSessionId = prevSessionId;
    }
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        unsub();
        reject(new Error(`Timeout waiting for ${method}`));
      }, timeoutMs);
      const unsub = this.onEvent((m, params, sessionId) => {
        if (m !== method) return;
        if (targetSessionId !== undefined && sessionId !== targetSessionId) return;
        if (predicate && !predicate(params as T)) return;
        clearTimeout(timer);
        unsub();
        resolve(params as T);
      });
    });
  }

  // ── Transport implementation ────────────────────────────────────────

  /**
   * Execute a CDP command with optional session scoping and timeout.
   *
   * Auto-heals: if the WebSocket dropped (transient hiccup, oversized
   * response, browser restart), reconnects once and retries the call
   * transparently. If the reconnect also fails, the error propagates.
   */
  _call(method: string, params: unknown = {}, timeoutOrOpts?: number | { sessionId?: string; timeoutMs?: number }): Promise<unknown> {
    // ── Auto-heal ──────────────────────────────────────────────────
    // If the WebSocket is dead, reconnect once and retry.
    // A single reconnect attempt prevents transient hiccups from
    // poisoning every subsequent call with "Not connected."
    if (!this.ws || this.ws.readyState !== WebSocket.OPEN) {
      if (this._reconnecting) {
        return Promise.reject(new Error('Not connected. Call session.connect(...) first.'));
      }
      this._reconnecting = true;
      const cleanup = () => { this._reconnecting = false; };
      return this.connect().then(() => {
        cleanup();
        return this._call(method, params, timeoutOrOpts);
      }, (e) => {
        cleanup();
        throw e;
      });
    }

    // ── Pending limit ──────────────────────────────────────────────
    if (this.pending.size >= Session.MAX_PENDING) {
      return Promise.reject(new Error('Too many pending CDP commands'));
    }

    // Parse 3rd argument: support legacy `number` (timeout) and new `object`
    let sessionIdOverride: string | undefined;
    let timeoutMs = 0;
    if (typeof timeoutOrOpts === 'number') {
      timeoutMs = timeoutOrOpts;
    } else if (timeoutOrOpts) {
      sessionIdOverride = timeoutOrOpts.sessionId;
      timeoutMs = timeoutOrOpts.timeoutMs ?? 0;
    }

    const id = this.nextId++;
    const msg: Record<string, unknown> = { id, method, params: params ?? {} };
    const sid = sessionIdOverride ?? this.activeSessionId;
    if (sid && !isBrowserLevel(method)) {
      msg.sessionId = sid;
    }

    // Pre-call observer (best-effort, never breaks the protocol call)
    if (this.recordCalls && this.callObserver) {
      try { this.callObserver(method, params); } catch { /* observer must never break protocol */ }
    }

    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws!.send(JSON.stringify(msg));

      if (timeoutMs > 0) {
        setTimeout(() => {
          const p = this.pending.get(id);
          if (p) {
            this.pending.delete(id);
            p.reject(new Error(`CDP command timed out after ${timeoutMs}ms: ${method}`));
          }
        }, timeoutMs);
      }
    });
  }

  private onMessage(raw: string): void {
    let m: any;
    try { m = JSON.parse(raw); } catch { return; }
    if (typeof m.id === 'number') {
      const p = this.pending.get(m.id);
      if (!p) return;
      this.pending.delete(m.id);
      if (m.error) p.reject(new CdpError(m.error.code, m.error.message, m.error.data));
      else p.resolve(m.result);
    } else if (m.method) {
      for (const fn of this.eventListeners) {
        try { fn(m.method, m.params, m.sessionId); } catch { /* ignore */ }
      }
    }
  }
}

export class CdpError extends Error {
  constructor(public code: number, message: string, public data?: unknown) {
    super(`CDP ${code}: ${message}`);
    this.name = 'CdpError';
  }
}

function isBrowserLevel(method: string): boolean {
  return method.startsWith('Browser.') || method.startsWith('Target.');
}

export async function resolveWsUrl(opts: ConnectOptions): Promise<string> {
  if (opts.wsUrl) return opts.wsUrl;
  if (opts.profileDir) {
    const { port, path } = await readDevToolsActivePort(opts.profileDir);
    return `ws://127.0.0.1:${port}${path}`;
  }
  throw new Error('resolveWsUrl needs { wsUrl } or { profileDir }.');
}

/** Best-effort browser name for the Dia-only auto-allow gate. */
async function browserNameFor(opts: ConnectOptions, wsUrl: string): Promise<string | undefined> {
  if (opts.profileDir) {
    const byDir = getBrowserCandidates().find(c => c.profileDir === opts.profileDir);
    if (byDir) return byDir.name;
  }
  const browsers = await detectBrowsers();
  return browsers.find(b => b.wsUrl === wsUrl || (opts.port != null && b.port === opts.port))?.name;
}

/**
 * Dismiss Dia's "Allow debugging connection?" prompt by sending Return to the
 * Dia process via osascript (macOS). Dia maps Return -> Allow; bringing Dia
 * to front first (best-effort try) covers the switched-away case. Gated on
 * name === 'Dia' in openWs, so the process name is hardcoded here. Needs
 * macOS Accessibility permission; without it osascript errors and the connect
 * just waits on its timeout. Uses child_process.execFile (Node.js-compatible)
 * for async fire-and-forget — works in both Bun and Node.js. */
function dismissDiaAllowPrompt(): void {
  if (process.platform !== 'darwin') return;
  try {
    execFile('osascript', [
      '-e', 'tell application "System Events"',
      '-e', 'try',
      '-e', 'set frontmost of process "Dia" to true',
      '-e', 'end try',
      '-e', 'tell process "Dia" to keystroke return',
      '-e', 'end tell',
    ], (err) => { /* ignore — best effort */ });
  } catch { /* spawn failure — best effort */ }
}

async function readDevToolsActivePort(profileDir: string): Promise<{ port: number; path: string }> {
  const deadline = Date.now() + 30_000;
  let lastErr: unknown;
  while (Date.now() < deadline) {
    try {
      const text = (await Bun.file(`${profileDir}/DevToolsActivePort`).text()).trim();
      const [portStr, path] = text.split('\n');
      const port = Number(portStr);
      if (!Number.isFinite(port)) throw new Error(`malformed port line: ${portStr}`);
      if (!path || !path.startsWith('/devtools/')) {
        throw new Error(`missing/invalid path line: ${JSON.stringify(text)}`);
      }
      return { port, path };
    } catch (e) {
      lastErr = e;
      await Bun.sleep(250);
    }
  }
  throw new Error(`Could not read ${profileDir}/DevToolsActivePort after 30s: ${lastErr}`);
}

export type PageTarget = { targetId: string; title: string; url: string; type: string };
export async function listPageTargets(session: Session): Promise<PageTarget[]> {
  const { targetInfos } = await session.domains.Target.getTargets({});
  return (targetInfos as PageTarget[]).filter(
    t => t.type === 'page' && !t.url.startsWith('chrome://') && !t.url.startsWith('devtools://')
  );
}

export async function detectBrowsers(): Promise<DetectedBrowser[]> {
  const detected: DetectedBrowser[] = [];
  const seenPorts = new Set<number>();

  // 1. DevToolsActivePort file scan (Google Chrome, Dia)
  for (const { name, profileDir } of getBrowserCandidates()) {
    const parsed = await tryReadDevToolsActivePort(profileDir);
    if (!parsed) continue;
    seenPorts.add(parsed.port);
    detected.push({
      name,
      profileDir,
      port: parsed.port,
      wsPath: parsed.path,
      wsUrl: `ws://127.0.0.1:${parsed.port}${parsed.path}`,
      mtimeMs: parsed.mtimeMs,
    });
  }

  // 2. Check port 9222 directly (custom instances, --remote-debugging-port)
  if (!seenPorts.has(9222)) {
    try {
      const resp = await fetch(`http://127.0.0.1:9222/json/version`, {
        signal: AbortSignal.timeout(800),
      });
      if (resp.ok) {
        const info = await resp.json() as Record<string, string>;
        detected.push({
          name: info.Browser || 'Chrome',
          profileDir: '',
          port: 9222,
          wsPath: info.webSocketDebuggerUrl.replace(/^ws:\/\/[^/]+/, ''),
          wsUrl: info.webSocketDebuggerUrl,
          mtimeMs: Date.now(),
          version: info.Browser,
        });
      }
    } catch { /* no browser on 9222 */ }
  }

  detected.sort((a, b) => b.mtimeMs - a.mtimeMs);
  return detected;
}

type BrowserCandidate = { name: string; profileDir: string };

/**
 * Returns known browser profile directories for the current platform.
 * Supports Google Chrome and Dia Browser (macOS). On other platforms,
 * auto-detection falls back to HTTP port scan (port 9222) or explicit
 * connection options.
 */
function getBrowserCandidates(): BrowserCandidate[] {
  const home = process.env.HOME ?? process.env.USERPROFILE ?? '';
  const list: BrowserCandidate[] = [];
  const push = (name: string, profileDir: string) => list.push({ name, profileDir });

  if (process.platform === 'darwin') {
    const base = `${home}/Library/Application Support`;
    push('Google Chrome', `${base}/Google/Chrome`);
    push('Dia',           `${base}/Dia`);
    push('Dia',           `${base}/Dia/User Data`);
  }
  return list;
}

async function tryReadDevToolsActivePort(
  profileDir: string,
): Promise<{ port: number; path: string; mtimeMs: number } | undefined> {
  try {
    const file = Bun.file(`${profileDir}/DevToolsActivePort`);
    const [text, mtimeMs] = await Promise.all([file.text(), file.lastModified]);
    const [portStr, path] = text.trim().split('\n');
    const port = Number(portStr);
    if (!Number.isFinite(port)) return undefined;
    if (!path || !path.startsWith('/devtools/')) return undefined;
    return { port, path, mtimeMs: mtimeMs as number };
  } catch {
    return undefined;
  }
}

async function discoverByPort(port: number): Promise<string> {
  // Method 1: HTTP endpoint discovery — try /json/version, /json, /json/list
  // Dia Browser returns 404 on /json/version (and all /json/*), so we fall
  // through to /json or /json/list which return an array with webSocketDebuggerUrl.
  const endpoints = ['/json/version', '/json', '/json/list'];
  for (const ep of endpoints) {
    try {
      const resp = await fetch(`http://127.0.0.1:${port}${ep}`, {
        signal: AbortSignal.timeout(2000),
      });
      if (!resp.ok) continue;
      const data = await resp.json();
      if (ep === '/json/version') {
        if (data.webSocketDebuggerUrl) return data.webSocketDebuggerUrl;
      } else if (Array.isArray(data) && data.length > 0) {
        if (data[0].webSocketDebuggerUrl) return data[0].webSocketDebuggerUrl;
      }
    } catch { /* try next endpoint */ }
  }

  // Method 2: Scan DevToolsActivePort files across known browsers
  for (const { profileDir } of getBrowserCandidates()) {
    try {
      const file = Bun.file(`${profileDir}/DevToolsActivePort`);
      const text = await file.text();
      const [p, wsPath] = text.trim().split('\n');
      if (Number(p) === port && wsPath?.startsWith('/devtools/')) {
        return `ws://127.0.0.1:${port}${wsPath}`;
      }
    } catch { /* no file or not matching */ }
  }

  throw new Error(
    `No browser on port ${port}. Enable CDP from chrome://inspect or dia://inspect, ` +
    `or start with --remote-debugging-port=${port}`,
  );
}
