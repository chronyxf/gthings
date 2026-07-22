/**
 * session.test.ts — Unit tests for the Session class
 *
 * Session wraps CDP-over-WebSocket communication. These tests validate
 * the class structure, invariants, and error handling WITHOUT requiring
 * a real CDP endpoint. WebSocket is mocked/stubbed where needed.
 *
 * Key coverage:
 *  - Constructor and structural invariants
 *  - Connection guard: isConnected() reflecting state
 *  - MAX_PENDING limit
 *  - _call rejection when not connected
 *  - _call timeout behavior (mock WebSocket)
 *  - activeSessionId lifecycle
 *  - Agent-tab registry (register/unregister/isAgentTab)
 *  - closeTab safety and serialization
 *  - cleanupAgentTabs lifecycle
 *  - Call observer (set/clear/invoke)
 *  - MAX_AGENT_TABS constant
 */

import { test, expect } from 'bun:test';
import { Session } from '../sdk/session.ts';

// ─── Constructor & Structural Invariants ─────────────────────────────────

test('constructor does not throw', () => {
  expect(() => new Session()).not.toThrow();
});

test('isConnected() returns false for a fresh instance', () => {
  const s = new Session();
  expect(s.isConnected()).toBe(false);
});

test('MAX_PENDING static constant exists and is 1000', () => {
  expect(Session.MAX_PENDING).toBeDefined();
  expect(typeof Session.MAX_PENDING).toBe('number');
  expect(Session.MAX_PENDING).toBe(1000);
});

test('activeSessionId starts as undefined', () => {
  const s = new Session();
  // activeSessionId is private; access via bracket or property access
  expect((s as any).activeSessionId).toBeUndefined();
});

test('Session exposes domains getter', () => {
  const s = new Session();
  expect(s.domains).toBeDefined();
  expect(typeof s.domains).toBe('object');
});

// ─── Connection Guard ─────────────────────────────────────────────────────

test('isConnected() returns false after close()', () => {
  const s = new Session();
  // close on a never-connected session should not throw
  expect(() => s.close()).not.toThrow();
  expect(s.isConnected()).toBe(false);
});

// ─── _call behavior ──────────────────────────────────────────────────────

test('_call rejects with "Not connected" when ws is undefined', async () => {
  const s = new Session();
  (s as any)._reconnecting = true; // bypass auto-heal to test guard directly
  await expect(s._call('TestMethod', {})).rejects.toThrow('Not connected. Call session.connect(...) first.');
});

test('_call rejects with "Not connected" when ws exists but is not OPEN', async () => {
  const s = new Session();
  // Set ws to a connecting (not open) state
  (s as any).ws = { readyState: 0 }; // WebSocket.CONNECTING = 0
  (s as any)._reconnecting = true; // bypass auto-heal to test guard directly
  await expect(s._call('TestMethod', {})).rejects.toThrow('Not connected. Call session.connect(...) first.');
});

test('_call with timeoutMs=1 rejects when no response arrives', async () => {
  const s = new Session();
  // Stub ws as connected so _call proceeds past the guard
  (s as any).ws = {
    readyState: 1, // WebSocket.OPEN = 1
    send: () => {}, // swallow sends — no CDP response will come
  };
  await expect(s._call('Test.method', {}, 1)).rejects.toThrow(
    'CDP command timed out after 1ms: Test.method',
  );
});

test('_call cleans up pending map after timeout', async () => {
  const s = new Session();
  (s as any).ws = {
    readyState: 1,
    send: () => {},
  };
  const pendingBefore = (s as any).pending.size;
  try {
    await s._call('Test.method', {}, 1);
  } catch {
    // expected
  }
  // After rejection, the pending entry should be cleaned up
  expect((s as any).pending.size).toBe(pendingBefore);
});

test('_call rejects with "Too many pending CDP commands" when pending is full', async () => {
  const s = new Session();
  (s as any).ws = {
    readyState: 1,
    send: () => {},
  };
  // Fill the pending map to MAX_PENDING
  const map: Map<number, { resolve: (v: unknown) => void; reject: (e: unknown) => void }> = (s as any).pending;
  for (let i = 0; i < Session.MAX_PENDING; i++) {
    map.set(i, { resolve: () => {}, reject: () => {} });
  }
  (s as any).nextId = Session.MAX_PENDING + 1;
  await expect(s._call('TestMethod', {})).rejects.toThrow('Too many pending CDP commands');
});

// ─── setActiveSession / getActiveSession ──────────────────────────────────

test('setActiveSession and getActiveSession round-trip', () => {
  const s = new Session();
  expect(s.getActiveSession()).toBeUndefined();
  s.setActiveSession('test-session-id');
  expect(s.getActiveSession()).toBe('test-session-id');
  s.setActiveSession(undefined);
  expect(s.getActiveSession()).toBeUndefined();
});

// ─── waitFor ──────────────────────────────────────────────────────────────

test('waitFor rejects on timeout when no event arrives', async () => {
  const s = new Session();
  // waitFor internally calls _call via this.use if targetId is provided,
  // so avoid targetId. With no events arriving, the timeout should fire.
  await expect(s.waitFor('Test.event', undefined, 10)).rejects.toThrow(
    'Timeout waiting for Test.event',
  );
});

// ─── Agent-tab registry ───────────────────────────────────────────────────

test('registerAgentTab adds a targetId to agentTabs', () => {
  const s = new Session();
  expect((s as any).agentTabs.size).toBe(0);
  s.registerAgentTab('tab-1');
  expect((s as any).agentTabs.has('tab-1')).toBe(true);
});

test('isAgentTab returns true for a registered targetId', () => {
  const s = new Session();
  s.registerAgentTab('tab-1');
  expect(s.isAgentTab('tab-1')).toBe(true);
});

test('isAgentTab returns false for an unknown targetId', () => {
  const s = new Session();
  expect(s.isAgentTab('unknown')).toBe(false);
});

test('isAgentTab returns false when no tabs are registered', () => {
  const s = new Session();
  expect(s.isAgentTab('anything')).toBe(false);
});

test('unregisterAgentTab removes a targetId from agentTabs', () => {
  const s = new Session();
  s.registerAgentTab('tab-1');
  expect(s.isAgentTab('tab-1')).toBe(true);
  s.unregisterAgentTab('tab-1');
  expect(s.isAgentTab('tab-1')).toBe(false);
});

test('registerAgentTab is idempotent — duplicate add keeps size 1', () => {
  const s = new Session();
  s.registerAgentTab('tab-1');
  s.registerAgentTab('tab-1');
  expect((s as any).agentTabs.size).toBe(1);
});

test('unregisterAgentTab on an unknown targetId does not throw', () => {
  const s = new Session();
  expect(() => s.unregisterAgentTab('unknown')).not.toThrow();
});

test('multiple agent tabs are all present after registration', () => {
  const s = new Session();
  s.registerAgentTab('tab-1');
  s.registerAgentTab('tab-2');
  s.registerAgentTab('tab-3');
  expect((s as any).agentTabs.size).toBe(3);
  expect(s.isAgentTab('tab-1')).toBe(true);
  expect(s.isAgentTab('tab-2')).toBe(true);
  expect(s.isAgentTab('tab-3')).toBe(true);
});

test('unregisterAgentTab removes only the specified tab', () => {
  const s = new Session();
  s.registerAgentTab('tab-1');
  s.registerAgentTab('tab-2');
  s.unregisterAgentTab('tab-1');
  expect(s.isAgentTab('tab-1')).toBe(false);
  expect(s.isAgentTab('tab-2')).toBe(true);
});

// ─── closeTab safety & behavior ───────────────────────────────────────────

test('closeTab unregisters the tab from agentTabs after close', async () => {
  const s = new Session();
  s.registerAgentTab('tab-1');
  expect(s.isAgentTab('tab-1')).toBe(true);
  // Without sessionId, closeTab skips Runtime.evaluate / Bun.sleep(100)
  // and only calls Target.closeTarget, which rejects (no WS) but the error
  // is caught silently. The unregister happens synchronously.
  await s.closeTab('tab-1');
  expect(s.isAgentTab('tab-1')).toBe(false);
});

test('closeTab without force on a non-agent tab throws — prevents accidental user tab closure', async () => {
  const s = new Session();
  // 'user-tab' was never registered, so the safety guard rejects.
  await expect(s.closeTab('user-tab', undefined, false)).rejects.toThrow(
    'is not an agent tab',
  );
});

test('closeTab with force:true on a non-registered tab succeeds — allows cleanup of stale tabs', async () => {
  const s = new Session();
  // force=true skips the agent-tab guard; the CDP call fails silently (no WS).
  await expect(s.closeTab('stale-tab', undefined, true)).resolves.toBeUndefined();
});

test('closeTab serializes close operations — second tab waits for first', async () => {
  const s = new Session();
  s.registerAgentTab('tab-1');
  s.registerAgentTab('tab-2');

  const closeOrder: string[] = [];

  // Override the Target.closeTarget domain method so we can observe the
  // order in which each tab's CDP close call actually executes.
  (s as any).domains.Target.closeTarget = async (params: { targetId: string }) => {
    await Bun.sleep(10);
    closeOrder.push(params.targetId);
  };

  // Fire both closeTab calls in the same synchronous tick.
  const p1 = s.closeTab('tab-1');
  const p2 = s.closeTab('tab-2');

  await Promise.all([p1, p2]);

  // Because closeTab serializes via the closeQueue promise chain, tab-1's
  // CDP close must complete before tab-2's starts.
  expect(closeOrder).toEqual(['tab-1', 'tab-2']);
});

// ─── cleanupAgentTabs ─────────────────────────────────────────────────────

test('cleanupAgentTabs closes all registered tabs and empties agentTabs', async () => {
  const s = new Session();
  s.registerAgentTab('tab-1');
  s.registerAgentTab('tab-2');
  s.registerAgentTab('tab-3');
  expect((s as any).agentTabs.size).toBe(3);
  // Prevent auto-heal from trying to connect a real WebSocket
  (s as any)._reconnecting = true;

  await s.cleanupAgentTabs();

  expect((s as any).agentTabs.size).toBe(0);
  expect(s.isAgentTab('tab-1')).toBe(false);
  expect(s.isAgentTab('tab-2')).toBe(false);
  expect(s.isAgentTab('tab-3')).toBe(false);
});

test('cleanupAgentTabs on an empty set does not throw', async () => {
  const s = new Session();
  await expect(s.cleanupAgentTabs()).resolves.toBeUndefined();
});

// ─── Call observer ────────────────────────────────────────────────────────

test('setCallObserver stores the callback', () => {
  const s = new Session();
  const fn = () => {};
  s.setCallObserver(fn);
  expect((s as any).callObserver).toBe(fn);
});

test('clearCallObserver removes the callback', () => {
  const s = new Session();
  s.setCallObserver(() => {});
  expect((s as any).callObserver).toBeDefined();
  s.clearCallObserver();
  expect((s as any).callObserver).toBeUndefined();
});

test('call observer is invoked with method and params on _call when recordCalls is true', async () => {
  const s = new Session();
  const observed: Array<{ method: string; params: unknown }> = [];
  s.setCallObserver((method, params) => {
    observed.push({ method, params });
  });
  (s as any).recordCalls = true;
  (s as any).ws = { readyState: 1, send: () => {} };

  // _call will time out, but the observer fires synchronously before the
  // Promise is created — so by the time we catch the rejection, observer
  // state is already captured.
  await expect(s._call('Test.method', { foo: 'bar' }, 1)).rejects.toThrow(
    'CDP command timed out after 1ms: Test.method',
  );

  expect(observed.length).toBe(1);
  expect(observed[0].method).toBe('Test.method');
  expect(observed[0].params).toEqual({ foo: 'bar' });
});

test('call observer is NOT invoked when recordCalls is false (default)', async () => {
  const s = new Session();
  let invoked = false;
  s.setCallObserver(() => {
    invoked = true;
  });
  // recordCalls defaults to false
  (s as any).ws = { readyState: 1, send: () => {} };

  await expect(s._call('Test.method', {}, 1)).rejects.toThrow(
    'CDP command timed out after 1ms: Test.method',
  );

  expect(invoked).toBe(false);
});

// ─── MAX_AGENT_TABS ───────────────────────────────────────────────────────

test('MAX_AGENT_TABS static constant equals Infinity (no built-in limit)', () => {
  expect(Session.MAX_AGENT_TABS).toBe(Infinity);
});

test('maxAgentTabs instance property defaults to Infinity', () => {
  const s = new Session();
  expect((s as any).maxAgentTabs).toBe(Infinity);
});
