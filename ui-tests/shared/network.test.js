import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

// network.js has side effects (addEventListener on window at module level),
// so we use vi.resetModules() and dynamic import to get a fresh module each test.

let isOnline, onNetworkChange, markOnline, OFFLINE_MESSAGE;

beforeEach(async () => {
  vi.resetModules();

  // Mock navigator.onLine before importing
  Object.defineProperty(navigator, 'onLine', { value: true, writable: true, configurable: true });

  // Mock fetch globally
  globalThis.fetch = vi.fn().mockResolvedValue({});

  // Mock window.addEventListener to capture handlers without side effects leaking
  const originalAddEventListener = window.addEventListener.bind(window);
  vi.spyOn(window, 'addEventListener').mockImplementation((event, handler) => {
    // Let the module register its handlers but don't actually add them to jsdom
    // to avoid cross-test pollution
    if (event === 'online' || event === 'offline') return;
    originalAddEventListener(event, handler);
  });

  const mod = await import('../../ui/js/shared/network.js');
  isOnline = mod.isOnline;
  onNetworkChange = mod.onNetworkChange;
  markOnline = mod.markOnline;
  OFFLINE_MESSAGE = mod.OFFLINE_MESSAGE;
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe('isOnline', () => {
  it('returns current online status', () => {
    // Module initializes _online from navigator.onLine which we set to true
    expect(isOnline()).toBe(true);
  });
});

describe('onNetworkChange', () => {
  it('registers a listener and returns unsubscribe function', () => {
    const listener = vi.fn();
    const unsub = onNetworkChange(listener);
    expect(typeof unsub).toBe('function');
  });

  it('listener is called when markOnline changes state', async () => {
    // First we need to get the module into an offline state
    // We'll reimport with navigator.onLine = false
    vi.resetModules();
    Object.defineProperty(navigator, 'onLine', { value: false, writable: true, configurable: true });
    vi.spyOn(window, 'addEventListener').mockImplementation(() => {});

    const mod = await import('../../ui/js/shared/network.js');
    const listener = vi.fn();
    mod.onNetworkChange(listener);

    // Now mark online — should trigger listener since state changes from false to true
    mod.markOnline();
    expect(listener).toHaveBeenCalledWith(true);
  });

  it('unsubscribe removes the listener', async () => {
    vi.resetModules();
    Object.defineProperty(navigator, 'onLine', { value: false, writable: true, configurable: true });
    vi.spyOn(window, 'addEventListener').mockImplementation(() => {});

    const mod = await import('../../ui/js/shared/network.js');
    const listener = vi.fn();
    const unsub = mod.onNetworkChange(listener);
    unsub();

    mod.markOnline();
    expect(listener).not.toHaveBeenCalled();
  });
});

describe('markOnline', () => {
  it('sets status to true', async () => {
    vi.resetModules();
    Object.defineProperty(navigator, 'onLine', { value: false, writable: true, configurable: true });
    vi.spyOn(window, 'addEventListener').mockImplementation(() => {});

    const mod = await import('../../ui/js/shared/network.js');
    expect(mod.isOnline()).toBe(false);
    mod.markOnline();
    expect(mod.isOnline()).toBe(true);
  });
});

describe('OFFLINE_MESSAGE', () => {
  it('is a non-empty string', () => {
    expect(typeof OFFLINE_MESSAGE).toBe('string');
    expect(OFFLINE_MESSAGE.length).toBeGreaterThan(0);
  });
});

describe('checkOnline', () => {
  async function freshModule({ onLine = true, fetchImpl } = {}) {
    vi.resetModules();
    Object.defineProperty(navigator, 'onLine', { value: onLine, writable: true, configurable: true });
    vi.spyOn(window, 'addEventListener').mockImplementation(() => {});
    globalThis.fetch = vi.fn(fetchImpl);
    return import('../../ui/js/shared/network.js');
  }

  afterEach(() => vi.useRealTimers());

  it('counts any one reachable probe as online', async () => {
    const mod = await freshModule({
      fetchImpl: async (url) => {
        if (url.includes('cloudflare')) throw new Error('blocked');
        return {};
      },
    });
    expect(await mod.checkOnline()).toBe(true);
    expect(globalThis.fetch.mock.calls.length).toBeGreaterThan(1);
  });

  it('needs two failed checks before reporting offline, then recovers by itself', async () => {
    vi.useFakeTimers();
    let up = false;
    const mod = await freshModule({
      fetchImpl: async () => {
        if (!up) throw new Error('no network yet');
        return {};
      },
    });
    const listener = vi.fn();
    mod.onNetworkChange(listener);

    // First failure at launch: not confirmed yet, no banner.
    expect(await mod.checkOnline()).toBe(true);
    expect(listener).not.toHaveBeenCalled();

    // The confirmation re-check also fails → offline.
    await vi.advanceTimersByTimeAsync(3_000);
    expect(mod.isOnline()).toBe(false);
    expect(listener).toHaveBeenLastCalledWith(false);

    // Network comes up; the scheduled retry clears it without an agent reply.
    up = true;
    await vi.advanceTimersByTimeAsync(5_000);
    expect(mod.isOnline()).toBe(true);
    expect(listener).toHaveBeenLastCalledWith(true);
  });

  it('trusts the OS saying offline immediately', async () => {
    const mod = await freshModule({
      onLine: false,
      fetchImpl: async () => {
        throw new Error('down');
      },
    });
    expect(await mod.checkOnline()).toBe(false);
  });

  it('shares one in-flight check between concurrent callers', async () => {
    const mod = await freshModule({ fetchImpl: async () => ({}) });
    const [a, b] = await Promise.all([mod.checkOnline(), mod.checkOnline()]);
    expect(a && b).toBe(true);
    expect(globalThis.fetch).toHaveBeenCalledTimes(3); // one round of probes
  });
});
