import { describe, it, expect, vi } from 'vitest';
import {
  processToolCallUpdate,
  extractSources,
  extractSourcesFromText,
  addSource,
  getSessionResetMessage,
  detectAutomationPlan,
  detectAutomationPlanIncremental,
  automationPlanToTasks,
  detectExtensionToolCall,
  detectExtensionToolCallIncremental,
  extractSuggestedActions,
  renderSourceChipsHtml,
  renderSourceBubblesHtml,
  renderToolChipsHtml,
  attachSourceClickHandler,
} from '../../ui/js/shared/streaming-utils.js';

// --- processToolCallUpdate ---

describe('processToolCallUpdate', () => {
  it('tracks tool usage from events', () => {
    const state = { toolUsages: [], toolSources: [] };
    const event = {
      payload: {
        params: {
          update: { toolCallId: 'tc1', title: 'read_file', kind: 'read' },
        },
      },
    };
    const result = processToolCallUpdate(event, state);
    expect(result.updated).toBe(true);
    expect(state.toolUsages).toHaveLength(1);
    expect(state.toolUsages[0]).toEqual({ toolCallId: 'tc1', title: 'read_file', kind: 'read' });
  });

  it('deduplicates by toolCallId', () => {
    const state = { toolUsages: [], toolSources: [] };
    const event = {
      payload: {
        params: {
          update: { toolCallId: 'tc1', title: 'read_file', kind: 'read' },
        },
      },
    };
    processToolCallUpdate(event, state);
    processToolCallUpdate(event, state);
    expect(state.toolUsages).toHaveLength(1);
  });

  it('returns updated false for missing update', () => {
    const state = { toolUsages: [], toolSources: [] };
    const result = processToolCallUpdate({ payload: {} }, state);
    expect(result.updated).toBe(false);
    expect(result.update).toBeNull();
  });

  it('returns updated false for null payload', () => {
    const state = { toolUsages: [], toolSources: [] };
    const result = processToolCallUpdate({ payload: null }, state);
    expect(result.updated).toBe(false);
  });
});

// --- extractSources ---

describe('extractSources', () => {
  it('extracts from items array with Json.results', () => {
    const state = { toolUsages: [], toolSources: [] };
    const rawOutput = {
      items: [
        { Json: { results: [{ url: 'https://example.com', title: 'Example', domain: 'example.com' }] } },
      ],
    };
    extractSources(rawOutput, state);
    expect(state.toolSources).toHaveLength(1);
    expect(state.toolSources[0].domain).toBe('example.com');
  });

  it('extracts from flat array', () => {
    const state = { toolUsages: [], toolSources: [] };
    extractSources([{ url: 'https://test.org/page', title: 'Test' }], state);
    expect(state.toolSources).toHaveLength(1);
    expect(state.toolSources[0].domain).toBe('test.org');
  });

  it('extracts from object with results key', () => {
    const state = { toolUsages: [], toolSources: [] };
    extractSources({ results: [{ url: 'https://foo.bar/baz', title: 'Foo' }] }, state);
    expect(state.toolSources).toHaveLength(1);
    expect(state.toolSources[0].domain).toBe('foo.bar');
  });

  it('extracts from object with searchResults key', () => {
    const state = { toolUsages: [], toolSources: [] };
    extractSources({ searchResults: [{ url: 'https://search.io', title: 'S' }] }, state);
    expect(state.toolSources).toHaveLength(1);
  });
});

// --- extractSourcesFromText ---

describe('extractSourcesFromText', () => {
  it('extracts markdown links from text', () => {
    const state = { toolUsages: [], toolSources: [] };
    extractSourcesFromText('Check [Google](https://google.com) and [GitHub](https://github.com)', state);
    expect(state.toolSources).toHaveLength(2);
    expect(state.toolSources[0].domain).toBe('google.com');
    expect(state.toolSources[1].domain).toBe('github.com');
  });

  it('ignores non-link text', () => {
    const state = { toolUsages: [], toolSources: [] };
    extractSourcesFromText('No links here at all', state);
    expect(state.toolSources).toHaveLength(0);
  });
});

// --- addSource ---

describe('addSource', () => {
  it('deduplicates by domain', () => {
    const state = { toolUsages: [], toolSources: [] };
    addSource('https://example.com/page1', 'Page 1', null, state);
    addSource('https://example.com/page2', 'Page 2', null, state);
    expect(state.toolSources).toHaveLength(1);
  });

  it('adds sources with different domains', () => {
    const state = { toolUsages: [], toolSources: [] };
    addSource('https://example.com', 'Ex', null, state);
    addSource('https://other.com', 'Other', null, state);
    expect(state.toolSources).toHaveLength(2);
  });

  it('strips www. from domain', () => {
    const state = { toolUsages: [], toolSources: [] };
    addSource('https://www.example.com', 'Ex', null, state);
    expect(state.toolSources[0].domain).toBe('example.com');
  });

  it('uses domainHint when provided', () => {
    const state = { toolUsages: [], toolSources: [] };
    addSource('https://cdn.example.com/img', 'Img', 'example.com', state);
    expect(state.toolSources[0].domain).toBe('example.com');
  });

  it('generates initials and color', () => {
    const state = { toolUsages: [], toolSources: [] };
    addSource('https://github.com/repo', 'GH', null, state);
    expect(state.toolSources[0].initials).toBe('GI');
    expect(state.toolSources[0].color).toMatch(/^hsl\(\d+, 55%, 45%\)$/);
  });

  it('ignores invalid URLs', () => {
    const state = { toolUsages: [], toolSources: [] };
    addSource('not-a-url', 'Bad', null, state);
    expect(state.toolSources).toHaveLength(0);
  });
});

// --- getSessionResetMessage ---

describe('getSessionResetMessage', () => {
  it('returns image unsupported message with reconnected', () => {
    const msg = getSessionResetMessage({ reason: 'image_unsupported', reconnected: true });
    expect(msg).toContain('support images');
    expect(msg).toContain('new session');
  });

  it('returns image unsupported message without reconnected', () => {
    const msg = getSessionResetMessage({ reason: 'image_unsupported', reconnected: false });
    expect(msg).toContain('support images');
    expect(msg).toContain('reconnect manually');
  });

  it('returns generic message for other reasons', () => {
    const msg = getSessionResetMessage({ reason: 'unknown' });
    expect(msg).toContain('reset due to an error');
  });

  it('returns generic message for null data', () => {
    const msg = getSessionResetMessage(null);
    expect(msg).toContain('reset due to an error');
  });
});


// --- detectAutomationPlan ---

describe('detectAutomationPlan', () => {
  it('parses a valid automation_plan block', () => {
    const text = 'Here is the plan:\n```automation_plan\n[{"step":1,"task":"Install deps","details":"npm install"}]\n```\nDone.';
    const plan = detectAutomationPlan(text);
    expect(plan).toHaveLength(1);
    expect(plan[0].task).toBe('Install deps');
  });

  it('returns null for missing block', () => {
    expect(detectAutomationPlan('No plan here')).toBeNull();
  });

  it('returns null for empty text', () => {
    expect(detectAutomationPlan('')).toBeNull();
    expect(detectAutomationPlan(null)).toBeNull();
  });

  it('returns null for invalid JSON in block', () => {
    const text = '```automation_plan\nnot json\n```';
    expect(detectAutomationPlan(text)).toBeNull();
  });

  it('returns null for valid JSON that is not an array with task', () => {
    const text = '```automation_plan\n{"key":"value"}\n```';
    expect(detectAutomationPlan(text)).toBeNull();
  });
});

// --- detectAutomationPlanIncremental ---

describe('detectAutomationPlanIncremental', () => {
  it('extracts steps from partial streaming text', () => {
    const text = '```automation_plan\n[{"step":1,"task":"Build project","details":"cargo build"},{"step":2,"task":"Run tests"';
    const steps = detectAutomationPlanIncremental(text);
    expect(steps).toHaveLength(1);
    expect(steps[0].step).toBe(1);
    expect(steps[0].task).toBe('Build project');
  });

  it('returns null when no automation_plan fence', () => {
    expect(detectAutomationPlanIncremental('just text')).toBeNull();
  });

  it('returns null for empty text', () => {
    expect(detectAutomationPlanIncremental('')).toBeNull();
    expect(detectAutomationPlanIncremental(null)).toBeNull();
  });

  it('extracts multiple complete steps', () => {
    const text = '```automation_plan\n[{"step":1,"task":"A","details":"a"},{"step":2,"task":"B","details":"b"}]';
    const steps = detectAutomationPlanIncremental(text);
    expect(steps).toHaveLength(2);
    expect(steps[1].task).toBe('B');
  });
});

// --- automationPlanToTasks ---

describe('automationPlanToTasks', () => {
  const plan = [
    { step: 1, task: 'Build', details: 'cargo build' },
    { step: 2, task: 'Test', details: 'cargo test' },
  ];

  it('maps pending status correctly', () => {
    const tasks = automationPlanToTasks(plan);
    expect(tasks).toHaveLength(2);
    expect(tasks[0].status).toBe('pending');
    expect(tasks[0].description).toBe('Build');
    expect(tasks[0].detail).toBe('cargo build');
  });

  it('maps running status to active', () => {
    const tasks = automationPlanToTasks(plan, { 1: 'running' });
    expect(tasks[0].status).toBe('active');
  });

  it('maps done status', () => {
    const tasks = automationPlanToTasks(plan, { 1: 'done' }, { 1: 'Success!' });
    expect(tasks[0].status).toBe('done');
    expect(tasks[0].detail).toBe('Success!');
  });

  it('maps failed status to error', () => {
    const tasks = automationPlanToTasks(plan, { 2: 'failed' });
    expect(tasks[1].status).toBe('error');
  });

  it('maps stopped status and sets cancelled', () => {
    const tasks = automationPlanToTasks(plan, { 1: 'stopped' });
    expect(tasks[0].status).toBe('stopped');
    expect(tasks[0].cancelled).toBe(true);
  });
});

// --- detectExtensionToolCall ---

describe('detectExtensionToolCall', () => {
  it('parses a complete extension_tool_call block', () => {
    const text = '```extension_tool_call\n{"extension":"my-ext","tool":"do_thing","params":{"a":1}}\n```';
    const result = detectExtensionToolCall(text);
    expect(result).toEqual({ extension: 'my-ext', tool: 'do_thing', params: { a: 1 } });
  });

  it('returns null for incomplete block (no closing fence)', () => {
    const text = '```extension_tool_call\n{"extension":"my-ext","tool":"do_thing"}';
    expect(detectExtensionToolCall(text)).toBeNull();
  });

  it('returns null for missing block', () => {
    expect(detectExtensionToolCall('no block')).toBeNull();
  });

  it('returns null for empty/null text', () => {
    expect(detectExtensionToolCall('')).toBeNull();
    expect(detectExtensionToolCall(null)).toBeNull();
  });

  it('defaults params to empty object', () => {
    const text = '```extension_tool_call\n{"extension":"e","tool":"t"}\n```';
    const result = detectExtensionToolCall(text);
    expect(result.params).toEqual({});
  });
});

// --- detectExtensionToolCallIncremental ---

describe('detectExtensionToolCallIncremental', () => {
  it('detects in-progress tool call', () => {
    const text = '```extension_tool_call\n{"extension":"my-ext","tool":"run"';
    const result = detectExtensionToolCallIncremental(text);
    expect(result).not.toBeNull();
    expect(result.inProgress).toBe(true);
    expect(result.extension).toBe('my-ext');
    expect(result.tool).toBe('run');
  });

  it('returns null for complete block (has closing fence)', () => {
    const text = '```extension_tool_call\n{"extension":"e","tool":"t"}\n```';
    expect(detectExtensionToolCallIncremental(text)).toBeNull();
  });

  it('returns null when no fence present', () => {
    expect(detectExtensionToolCallIncremental('plain text')).toBeNull();
  });

  it('returns null for empty/null', () => {
    expect(detectExtensionToolCallIncremental('')).toBeNull();
    expect(detectExtensionToolCallIncremental(null)).toBeNull();
  });

  it('extracts partial info via regex when JSON is incomplete', () => {
    const text = '```extension_tool_call\n{"extension":"partial"';
    const result = detectExtensionToolCallIncremental(text);
    expect(result.inProgress).toBe(true);
    expect(result.extension).toBe('partial');
  });
});

// --- extractSuggestedActions ---

describe('extractSuggestedActions', () => {
  it('parses a valid suggested_actions block', () => {
    const text = 'Some text\n```suggested_actions\n[{"label":"Fix it","prompt":"fix the bug"}]\n```\nMore text';
    const result = extractSuggestedActions(text);
    expect(result).not.toBeNull();
    expect(result.actions).toHaveLength(1);
    expect(result.actions[0].label).toBe('Fix it');
  });

  it('strips the block from cleanText', () => {
    const text = 'Before\n```suggested_actions\n[{"label":"A","prompt":"B"}]\n```\nAfter';
    const result = extractSuggestedActions(text);
    expect(result.cleanText).toContain('Before');
    expect(result.cleanText).toContain('After');
    expect(result.cleanText).not.toContain('suggested_actions');
  });

  it('returns null for missing block', () => {
    expect(extractSuggestedActions('no actions')).toBeNull();
  });

  it('returns null for empty/null text', () => {
    expect(extractSuggestedActions('')).toBeNull();
    expect(extractSuggestedActions(null)).toBeNull();
  });

  it('returns null for invalid JSON', () => {
    const text = '```suggested_actions\nnot json\n```';
    expect(extractSuggestedActions(text)).toBeNull();
  });

  it('returns null for array without label field', () => {
    const text = '```suggested_actions\n[{"foo":"bar"}]\n```';
    expect(extractSuggestedActions(text)).toBeNull();
  });
});

// --- renderSourceChipsHtml / renderSourceBubblesHtml: XSS hardening ---
//
// Source URLs/titles/colors come from agent-streamed search results and
// markdown links — i.e. attacker-influenceable content. The render output
// must never interpolate them into a JS context, and HTML attributes must
// be escaped so an injected `"` can't introduce new attributes.
//
// We assert via parsed-DOM, not raw-string regex: HTML-escaped content like
// `&lt;img onerror=…&gt;` shows up as inert text, but a regex against the
// raw string would still match "onerror=". The browser is the source of truth.

function parseHtml(html) {
  const root = document.createElement('div');
  root.innerHTML = html;
  return root;
}

/** Collect every attribute name across every element under root. */
function allAttributeNames(root) {
  const names = [];
  for (const el of root.querySelectorAll('*')) {
    for (const attr of el.attributes) names.push(attr.name);
  }
  return names;
}

describe('renderSourceChipsHtml — no inline JS, attribute injection neutralized', () => {
  it('does not emit an inline onclick attribute', () => {
    const html = renderSourceChipsHtml([
      { url: 'https://example.com', domain: 'example.com', title: 't', initials: 'EX', color: 'red', favicon: 'fav' },
    ]);
    const root = parseHtml(html);
    expect(allAttributeNames(root)).not.toContain('onclick');
  });

  it('preserves URL with backslash + single-quote in data-url (no breakout)', () => {
    // Pre-fix code did `url.replace(/'/g, "\\'")` and inlined into a JS
    // string literal — a backslash before a single-quote could collapse the
    // escape and break out into an inline-JS context. With data-url + HTML
    // escaping, both characters become attribute-safe.
    const malicious = "https://x.com/?q=a\\')+alert(1)+(';";
    const html = renderSourceChipsHtml([
      { url: malicious, domain: 'x', title: 't', initials: 'X', color: 'red', favicon: '' },
    ]);
    const root = parseHtml(html);
    const link = root.querySelector('.source-chip');
    // The URL is preserved verbatim in data-url (round-trips intact).
    expect(link.getAttribute('data-url')).toBe(malicious);
    // No event handlers introduced anywhere.
    const handlerAttrs = allAttributeNames(root).filter((n) => n.startsWith('on'));
    // Only the safe img onload/onerror from the template.
    expect(handlerAttrs.sort()).toEqual(['onerror', 'onload']);
  });

  it('does not introduce new event handlers via title injection', () => {
    const html = renderSourceChipsHtml([
      { url: 'https://x', domain: 'x', title: 'a" onerror="alert(1)', initials: 'X', color: 'red', favicon: '' },
    ]);
    const root = parseHtml(html);
    // Title is inert text on the link; no onerror on the link itself.
    const link = root.querySelector('.source-chip');
    expect(link.getAttribute('onerror')).toBeNull();
    expect(link.getAttribute('title')).toContain('alert(1)'); // inert
  });

  it('does not introduce new event handlers via color injection', () => {
    const html = renderSourceChipsHtml([
      { url: 'https://x', domain: 'x', title: 't', initials: 'X', color: 'red";onerror="alert(1)', favicon: '' },
    ]);
    const root = parseHtml(html);
    const initials = root.querySelector('.source-initials');
    expect(initials.getAttribute('onerror')).toBeNull();
  });

  it('does not introduce new event handlers via favicon injection', () => {
    const html = renderSourceChipsHtml([
      { url: 'https://x', domain: 'x', title: 't', initials: 'X', color: 'red', favicon: '" onerror="alert(1)' },
    ]);
    const root = parseHtml(html);
    // The img has its safe template onload/onerror. No injected onmouseover etc.
    const handlers = allAttributeNames(root).filter((n) => n.startsWith('on'));
    expect(handlers.sort()).toEqual(['onerror', 'onload']);
  });
});

describe('renderSourceBubblesHtml — same hardening as chips', () => {
  it('does not emit an inline onclick attribute', () => {
    const html = renderSourceBubblesHtml(
      [{ toolCallId: 'a', title: 'tool', kind: 'read' }],
      [{ url: 'https://x', domain: 'x', title: 't', initials: 'X', color: 'red', favicon: '' }],
    );
    const root = parseHtml(html);
    expect(allAttributeNames(root)).not.toContain('onclick');
  });

  it('places the URL in a data-url attribute (round-trips intact)', () => {
    const url = 'https://x.com?a="&b=c';
    const html = renderSourceBubblesHtml(
      [],
      [{ url, domain: 'x', title: 't', initials: 'X', color: 'r', favicon: '' }],
    );
    const root = parseHtml(html);
    const link = root.querySelector('.source-bubble');
    // The browser unescapes the attribute; the original URL must round-trip.
    expect(link.getAttribute('data-url')).toBe(url);
  });
});

describe('renderToolChipsHtml — defensive escaping', () => {
  it('treats an injected <img onerror=…> in extension tool title as inert text', () => {
    // Extension tool titles look like `ext:<name>/<tool>` — the regex split
    // here would pass an attacker-controlled name straight into the chip
    // body. Render must escape it.
    const html = renderToolChipsHtml([
      { toolCallId: 'a', title: 'ext:<img src=x onerror=alert(1)>/foo', kind: 'read' },
    ]);
    const root = parseHtml(html);
    // No real <img> element should exist (escaping must collapse it to text).
    expect(root.querySelector('img')).toBeNull();
    // No event handlers anywhere.
    expect(allAttributeNames(root).filter((n) => n.startsWith('on'))).toEqual([]);
  });
});

describe('attachSourceClickHandler — delegated open_url routing', () => {
  it('routes clicks on data-url elements through invoke(open_url)', () => {
    document.body.innerHTML = `
      <div id="container">
        <a class="source-chip" data-url="https://example.com"><span class="source-domain">example.com</span></a>
      </div>
    `;
    const container = document.getElementById('container');
    const invoke = vi.fn(() => Promise.resolve());
    attachSourceClickHandler(container, invoke);

    container.querySelector('.source-chip').click();
    expect(invoke).toHaveBeenCalledWith('open_url', { url: 'https://example.com' });
  });

  it('routes clicks on inner spans up to the data-url ancestor', () => {
    document.body.innerHTML = `
      <div id="container">
        <a class="source-chip" data-url="https://nested.test"><span class="source-domain">nested</span></a>
      </div>
    `;
    const container = document.getElementById('container');
    const invoke = vi.fn(() => Promise.resolve());
    attachSourceClickHandler(container, invoke);

    container.querySelector('.source-domain').click();
    expect(invoke).toHaveBeenCalledWith('open_url', { url: 'https://nested.test' });
  });

  it('is idempotent — installing twice does not double-fire', () => {
    document.body.innerHTML = `
      <div id="container">
        <a class="source-chip" data-url="https://once.test"></a>
      </div>
    `;
    const container = document.getElementById('container');
    const invoke = vi.fn(() => Promise.resolve());
    attachSourceClickHandler(container, invoke);
    attachSourceClickHandler(container, invoke);

    container.querySelector('.source-chip').click();
    expect(invoke).toHaveBeenCalledTimes(1);
  });

  it('ignores clicks outside any data-url element', () => {
    document.body.innerHTML = `
      <div id="container">
        <span class="not-a-link">just text</span>
      </div>
    `;
    const container = document.getElementById('container');
    const invoke = vi.fn(() => Promise.resolve());
    attachSourceClickHandler(container, invoke);

    container.querySelector('.not-a-link').click();
    expect(invoke).not.toHaveBeenCalled();
  });
});
