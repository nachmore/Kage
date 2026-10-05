import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { TtsStreamer } from '../../ui/js/shared/tts-streamer.js';

// Capture enqueued sentences instead of hitting the TTS server.
function makeStreamer() {
  const s = new TtsStreamer({ port: 0, voice: 'v', barContainer: null });
  const sent = [];
  s._enqueueSentence = (sentence) => sent.push(sentence);
  s._playNext = () => {};
  return { s, sent };
}

describe('TtsStreamer streaming commits', () => {
  it('keeps streaming past a stray inline backtick', () => {
    const { s, sent } = makeStreamer();
    const text =
      'Press the ` key to open the console.\n\nThen type the command.\n\n' +
      'After that, wait a bit.\n\nNow more prose here.';
    s.feedText(text);
    expect(sent.length).toBeGreaterThan(0);
    expect(sent.join(' ')).toContain('Then type the command.');
  });

  it('reads a reset accumulator from its start', () => {
    const { s, sent } = makeStreamer();
    s.feedText('Let me check the weather for you.\n```extension_tool_call');
    expect(sent.join(' ')).toContain('Let me check the weather for you.');
    sent.length = 0;

    // Accumulator reset after the tool call; follow-up streams from scratch.
    const followUp = "It's 18 degrees and sunny in Seattle today. Enjoy it.";
    s.feedText(followUp.slice(0, 10));
    s.finishText(followUp);
    expect(sent.join(' ')).toContain("It's 18 degrees and sunny in Seattle today.");
  });

  it('detects a reset even when the first new chunk is longer than the offset', () => {
    const { s, sent } = makeStreamer();
    s.feedText('Let me check that for you.\n');
    sent.length = 0;
    const followUp = 'Here is a much longer follow-up reply that exceeds the offset.';
    s.finishText(followUp);
    expect(sent.join(' ')).toContain('Here is a much longer follow-up');
  });

  it('does not re-send already committed text as the accumulator grows', () => {
    const { s, sent } = makeStreamer();
    s.feedText('First sentence is long enough.\n');
    s.finishText('First sentence is long enough.\nSecond sentence follows here.');
    expect(sent.filter((x) => x.includes('First sentence')).length).toBe(1);
    expect(sent.join(' ')).toContain('Second sentence follows here.');
  });
});

// ─── Playback queue / completion ───

// Fake Audio: tests drive `onended` / `onerror` / play() rejection by hand.
class FakeAudio {
  static instances = [];
  static playImpl = () => Promise.resolve();
  constructor(src) {
    this.src = src;
    this.onended = null;
    this.onerror = null;
    FakeAudio.instances.push(this);
  }
  play() {
    return FakeAudio.playImpl(this);
  }
  pause() {}
}

// Deferred fetch: every /tts request is held until the test settles it.
function installFetch() {
  const calls = [];
  const fetchMock = vi.fn((url, opts = {}) => {
    if (String(url).endsWith('/stop')) return Promise.resolve({ ok: true });
    return new Promise((resolve, reject) => {
      const call = { url, opts, resolve, reject };
      opts.signal?.addEventListener('abort', () =>
        reject(new DOMException('aborted', 'AbortError'))
      );
      calls.push(call);
    });
  });
  vi.stubGlobal('fetch', fetchMock);
  return { calls, fetchMock };
}

const okAudio = () => ({
  ok: true,
  status: 200,
  headers: { get: () => 'audio/wav' },
  blob: async () => new Blob(['x']),
});
const serverError = () => ({
  ok: false,
  status: 500,
  headers: { get: () => null },
  json: async () => ({ error: 'boom' }),
});

// Let the async dispatch loop run its pending continuations.
const flush = async () => {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0));
};

const TWO_SENTENCES = 'This is the first sentence here. This is the second sentence here.';

describe('TtsStreamer playback completion', () => {
  let origCreate;
  let origRevoke;

  beforeEach(() => {
    FakeAudio.instances = [];
    FakeAudio.playImpl = () => Promise.resolve();
    vi.stubGlobal('Audio', FakeAudio);
    origCreate = URL.createObjectURL;
    origRevoke = URL.revokeObjectURL;
    let n = 0;
    URL.createObjectURL = () => `blob:${++n}`;
    URL.revokeObjectURL = () => {};
  });

  afterEach(() => {
    vi.unstubAllGlobals();
    URL.createObjectURL = origCreate;
    URL.revokeObjectURL = origRevoke;
  });

  function makePlayer() {
    const onFinished = vi.fn();
    const s = new TtsStreamer({ port: 1, voice: 'v', barContainer: null, onFinished });
    return { s, onFinished };
  }

  it('fires onFinished when the last fetch fails after earlier audio played', async () => {
    const { calls } = installFetch();
    const { s, onFinished } = makePlayer();
    s.finishText(TWO_SENTENCES);
    await flush();
    expect(calls).toHaveLength(1);

    calls[0].resolve(okAudio());
    await flush();
    expect(FakeAudio.instances).toHaveLength(1);
    expect(calls).toHaveLength(2);

    // First chunk finishes while the second fetch is still in flight.
    FakeAudio.instances[0].onended();
    expect(onFinished).not.toHaveBeenCalled();

    calls[1].resolve(serverError());
    await flush();
    expect(onFinished).toHaveBeenCalledTimes(1);
    expect(s.isActive).toBe(false);
  });

  it('fires onFinished once when every fetch fails', async () => {
    const { calls } = installFetch();
    const { s, onFinished } = makePlayer();
    s.finishText(TWO_SENTENCES);
    await flush();
    calls[0].resolve(serverError());
    await flush();
    // First failure must not complete early while the second is still queued.
    expect(onFinished).not.toHaveBeenCalled();
    calls[1].resolve(serverError());
    await flush();
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it('skips a failed middle chunk and still plays the next one', async () => {
    const { calls } = installFetch();
    const { s, onFinished } = makePlayer();
    s.finishText(
      'This is the first sentence here. This is the second sentence here. ' +
        'This is the third sentence here.'
    );
    await flush();
    calls[0].resolve(okAudio());
    await flush();
    FakeAudio.instances[0].onended();
    calls[1].resolve(serverError());
    await flush();
    expect(onFinished).not.toHaveBeenCalled();
    calls[2].resolve(okAudio());
    await flush();
    expect(FakeAudio.instances).toHaveLength(2);
    FakeAudio.instances[1].onended();
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it('advances once when a decode error both fires onerror and rejects play()', async () => {
    const { calls } = installFetch();
    let rejectPlay;
    FakeAudio.playImpl = () =>
      new Promise((_, reject) => {
        rejectPlay = reject;
      });
    const { s, onFinished } = makePlayer();
    s.finishText(TWO_SENTENCES);
    await flush();
    calls[0].resolve(okAudio());
    await flush();
    calls[1].resolve(okAudio());
    await flush();
    expect(FakeAudio.instances).toHaveLength(1);

    FakeAudio.playImpl = () => Promise.resolve();
    FakeAudio.instances[0].onerror();
    rejectPlay(new DOMException('decode', 'NotSupportedError'));
    await flush();
    // The second chunk plays exactly once; it isn't skipped by the
    // duplicate failure signal.
    expect(FakeAudio.instances).toHaveLength(2);
    expect(onFinished).not.toHaveBeenCalled();
    FakeAudio.instances[1].onended();
    expect(onFinished).toHaveBeenCalledTimes(1);
  });

  it('does not fire onFinished when stopped mid-fetch, nor retry the abort', async () => {
    const { calls, fetchMock } = installFetch();
    const { s, onFinished } = makePlayer();
    s.finishText(TWO_SENTENCES);
    await flush();
    calls[0].resolve(okAudio());
    await flush();
    expect(calls).toHaveLength(2);

    s.stop();
    // stop() blanks the element's src, which raises an error event.
    FakeAudio.instances[0].onerror?.();
    await flush();
    expect(onFinished).not.toHaveBeenCalled();
    // Only the two /tts requests plus the /stop call; the aborted fetch
    // didn't schedule a retry.
    expect(calls).toHaveLength(2);
    expect(fetchMock.mock.calls.filter(([u]) => String(u).endsWith('/stop'))).toHaveLength(1);
    expect(s.isActive).toBe(false);
  });

  it('does not fire onFinished again when stopped after completion', async () => {
    const { calls } = installFetch();
    const { s, onFinished } = makePlayer();
    s.finishText('This is a single sentence to speak.');
    await flush();
    calls[0].resolve(okAudio());
    await flush();
    FakeAudio.instances[0].onended();
    expect(onFinished).toHaveBeenCalledTimes(1);
    s.stop();
    FakeAudio.instances[0].onerror?.();
    s.finishText('This is a single sentence to speak.');
    await flush();
    expect(onFinished).toHaveBeenCalledTimes(1);
  });
});
