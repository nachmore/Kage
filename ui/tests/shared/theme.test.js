import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { setupTauriMock, teardownTauriMock } from '../helpers/tauri-mock.js';

// theme.js uses ES module exports — import after mocking
let applyTheme, loadAndApplyTheme, initThemeListener;

beforeEach(async () => {
  // Reset module state between tests
  vi.resetModules();

  // Mock matchMedia before importing theme.js
  window.matchMedia = vi.fn().mockReturnValue({
    matches: false,
    addEventListener: vi.fn(),
    removeEventListener: vi.fn(),
  });

  const mod = await import('../../js/shared/theme.js');
  applyTheme = mod.applyTheme;
  loadAndApplyTheme = mod.loadAndApplyTheme;
  initThemeListener = mod.initThemeListener;
});

afterEach(() => {
  teardownTauriMock();
  document.body.className = '';
  document.documentElement.style.cssText = '';
});

describe('applyTheme', () => {
  it('sets light-theme class for "light"', () => {
    applyTheme('light');
    expect(document.body.classList.contains('light-theme')).toBe(true);
    expect(document.body.classList.contains('dark-theme')).toBe(false);
  });

  it('sets dark-theme class for "dark"', () => {
    applyTheme('dark');
    expect(document.body.classList.contains('dark-theme')).toBe(true);
    expect(document.body.classList.contains('light-theme')).toBe(false);
  });

  it('uses cached OS dark mode for "system"', () => {
    // Default cachedOsDarkMode is from matchMedia mock (false = light)
    applyTheme('system');
    expect(document.body.classList.contains('light-theme')).toBe(true);
    expect(document.body.classList.contains('dark-theme')).toBe(false);
  });

  it('toggles correctly when switching themes', () => {
    applyTheme('dark');
    expect(document.body.classList.contains('dark-theme')).toBe(true);

    applyTheme('light');
    expect(document.body.classList.contains('light-theme')).toBe(true);
    expect(document.body.classList.contains('dark-theme')).toBe(false);

    applyTheme('dark');
    expect(document.body.classList.contains('dark-theme')).toBe(true);
    expect(document.body.classList.contains('light-theme')).toBe(false);
  });
});

describe('loadAndApplyTheme', () => {
  it('reads theme from config and applies it', async () => {
    const { invoke } = setupTauriMock({
      get_config: () => ({ ui: { theme: 'dark', font_size: 14 } }),
      get_os_dark_mode: () => true,
    });

    await loadAndApplyTheme(invoke);

    expect(document.body.classList.contains('dark-theme')).toBe(true);
    expect(invoke).toHaveBeenCalledWith('get_config');
    expect(invoke).toHaveBeenCalledWith('get_os_dark_mode');
  });

  it('applies light theme from config', async () => {
    const { invoke } = setupTauriMock({
      get_config: () => ({ ui: { theme: 'light', font_size: 14 } }),
      get_os_dark_mode: () => true,
    });

    await loadAndApplyTheme(invoke);

    expect(document.body.classList.contains('light-theme')).toBe(true);
    expect(document.body.classList.contains('dark-theme')).toBe(false);
  });

  it('uses OS dark mode for system theme', async () => {
    const { invoke } = setupTauriMock({
      get_config: () => ({ ui: { theme: 'system' } }),
      get_os_dark_mode: () => true,
    });

    await loadAndApplyTheme(invoke);
    expect(document.body.classList.contains('dark-theme')).toBe(true);
  });

  it('uses OS light mode for system theme', async () => {
    const { invoke } = setupTauriMock({
      get_config: () => ({ ui: { theme: 'system' } }),
      get_os_dark_mode: () => false,
    });

    await loadAndApplyTheme(invoke);
    expect(document.body.classList.contains('light-theme')).toBe(true);
  });

  it('loads custom theme colors', async () => {
    const { invoke } = setupTauriMock({
      get_config: () => ({ ui: { theme: 'kiro-ish-theme' } }),
      get_os_dark_mode: () => true,
      load_theme_colors: ({ themeId, variant }) => ({
        'kage-accent': '#7138CC',
        'kage-bg': '#1e1e1e',
      }),
    });

    await loadAndApplyTheme(invoke);

    expect(invoke).toHaveBeenCalledWith('load_theme_colors', {
      themeId: 'kiro-ish-theme',
      variant: 'dark',
    });
    expect(document.documentElement.style.getPropertyValue('--kage-accent')).toBe('#7138CC');
    expect(document.documentElement.style.getPropertyValue('--kage-bg')).toBe('#1e1e1e');
  });

  it('clears custom colors when switching to builtin theme', async () => {
    // First apply a custom theme
    const { invoke } = setupTauriMock({
      get_config: () => ({ ui: { theme: 'kiro-ish-theme' } }),
      get_os_dark_mode: () => true,
      load_theme_colors: () => ({ 'kage-accent': '#7138CC' }),
    });
    await loadAndApplyTheme(invoke);
    expect(document.documentElement.style.getPropertyValue('--kage-accent')).toBe('#7138CC');

    // Now switch to builtin
    invoke.mockImplementation(async (cmd) => {
      if (cmd === 'get_config') return { ui: { theme: 'dark' } };
      if (cmd === 'get_os_dark_mode') return true;
      throw new Error(`Unexpected: ${cmd}`);
    });
    await loadAndApplyTheme(invoke);
    expect(document.documentElement.style.getPropertyValue('--kage-accent')).toBe('');
  });

  it('applies font size from config', async () => {
    const { invoke } = setupTauriMock({
      get_config: () => ({ ui: { theme: 'dark', font_size: 18 } }),
      get_os_dark_mode: () => true,
    });

    await loadAndApplyTheme(invoke);
    expect(document.documentElement.style.getPropertyValue('--app-font-size')).toBe('18px');
  });

  it('falls back to system on error', async () => {
    const { invoke } = setupTauriMock({
      get_config: () => { throw new Error('fail'); },
    });

    await loadAndApplyTheme(invoke);
    // Should not throw, should apply system default
    expect(
      document.body.classList.contains('dark-theme') ||
      document.body.classList.contains('light-theme')
    ).toBe(true);
  });
});
