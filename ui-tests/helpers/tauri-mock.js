/**
 * Mock Tauri APIs for testing.
 * Call setupTauriMock() in beforeEach to install, teardown in afterEach.
 */

export function setupTauriMock(invokeHandlers = {}) {
  const listeners = {};

  const invoke = vi.fn(async (cmd, args) => {
    if (invokeHandlers[cmd]) return invokeHandlers[cmd](args);
    throw new Error(`Unhandled invoke: ${cmd}`);
  });

  const listen = vi.fn(async (event, handler) => {
    if (!listeners[event]) listeners[event] = [];
    listeners[event].push(handler);
    return () => {
      listeners[event] = listeners[event].filter(h => h !== handler);
    };
  });

  const emit = vi.fn((event, payload) => {
    (listeners[event] || []).forEach(h => h({ payload }));
  });

  window.__TAURI__ = {
    core: { invoke },
    event: { listen, emit },
    webviewWindow: {
      getCurrentWebviewWindow: () => ({
        show: vi.fn(),
        hide: vi.fn(),
        setFocus: vi.fn(),
      }),
    },
  };

  return { invoke, listen, emit, listeners };
}

export function teardownTauriMock() {
  delete window.__TAURI__;
}
