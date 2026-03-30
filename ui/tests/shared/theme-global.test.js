import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';
import { readFileSync } from 'fs';
import { resolve } from 'path';

// theme-global.js is a plain script that sets window.kageTheme
// We load it by evaluating its source in the jsdom context
function loadThemeGlobal() {
  const src = readFileSync(
    resolve(__dirname, '../../js/shared/theme-global.js'),
    'utf-8'
  );
  // Execute in the global scope
  const fn = new Function(src);
  fn();
}

beforeEach(() => {
  window.matchMedia = vi.fn().mockReturnValue({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  });
  delete window.kageTheme;
  loadThemeGlobal();
});

afterEach(() => {
  teardownTauriMock();
  document.body.className = '';
  document.documentElement.style.cssText = '';
  delete window.kageTheme;
});

describe('kageTheme', () => {
  it('exposes init, refresh, applyClasses, isDark on window', () => {
    expect(window.kageTheme).toBeDefined();
    expect(typeof window.kageTheme.init).toBe('function');
    expect(typeof window.kageTheme.refresh).toBe('function');
    expect(typeof window.kageTheme.applyClasses).toBe('function');
    expect(typeof window.kageTheme.isDark).toBe('function');
  });

  describe('applyClasses', () => {
    it('sets light-theme for "light"', () => {
      window.kageTheme.applyClasses('light');
      expect(document.body.classList.contains('light-theme')).toBe(true);
      expect(document.body.classList.contains('dark-theme')).toBe(false);
    });

    it('sets dark-theme for "dark"', () => {
      window.kageTheme.applyClasses('dark');
      expect(document.body.classList.contains('dark-theme')).toBe(true);
      expect(document.body.classList.contains('light-theme')).toBe(false);
    });
  });

  describe('init', () => {
    it('queries backend and applies theme', async () => {
      setupTauriMock({
        get_os_dark_mode: () => true,
        get_config: () => ({ ui: { theme: 'dark' } }),
      });

      window.kageTheme.init();
      // init is async internally — give it a tick
      await new Promise(r => setTimeout(r, 50));

      expect(document.body.classList.contains('dark-theme')).toBe(true);
    });

    it('loads custom theme colors', async () => {
      setupTauriMock({
        get_os_dark_mode: () => true,
        get_config: () => ({ ui: { theme: 'sunset-theme' } }),
        load_theme_colors: () => ({ 'kage-accent': '#E8853D' }),
      });

      window.kageTheme.init();
      await new Promise(r => setTimeout(r, 50));

      expect(document.documentElement.style.getPropertyValue('--kage-accent')).toBe('#E8853D');
    });
  });

  describe('refresh', () => {
    it('accepts theme override without reading config', async () => {
      const { invoke } = setupTauriMock({
        get_os_dark_mode: () => false,
      });

      // Must init first so _invoke is set
      window.kageTheme.init();
      await new Promise(r => setTimeout(r, 50));

      // Reset mock call history, then refresh with override
      invoke.mockClear();
      await window.kageTheme.refresh('light');

      expect(document.body.classList.contains('light-theme')).toBe(true);
      // Should NOT have called get_config since we passed an override
      expect(invoke).not.toHaveBeenCalledWith('get_config');
    });
  });
});
