import { describe, it, expect } from 'vitest';
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
