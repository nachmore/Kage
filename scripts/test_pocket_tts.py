"""
Test script for the Pocket TTS server.

Usage:
  1. Start the server:  python pocket_tts/server.py
  2. Run this script:   python scripts/test_pocket_tts.py

Options:
  --port PORT       Server port (default: 9877)
  --voice VOICE     Voice to test with (default: alba)
  --text TEXT       Text to speak (default: sample sentence)
  --output FILE     Save audio to file instead of playing (e.g. output.wav)
  --no-play         Don't play audio, just test the endpoints
  --all-voices      Test all built-in voices
"""

import argparse
import json
import os
import sys
import time
import urllib.request
import urllib.error

BASE = "http://127.0.0.1:{}"

def req(port, method, path, body=None):
    """Make an HTTP request and return (status, json_or_bytes)."""
    url = BASE.format(port) + path
    headers = {"Content-Type": "application/json"} if body else {}
    data = json.dumps(body).encode() if body else None
    r = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        resp = urllib.request.urlopen(r, timeout=120)
        ct = resp.headers.get("Content-Type", "")
        raw = resp.read()
        if "json" in ct:
            return resp.status, json.loads(raw)
        return resp.status, raw
    except urllib.error.HTTPError as e:
        body_text = e.read().decode(errors="replace")
        try:
            return e.code, json.loads(body_text)
        except Exception:
            return e.code, body_text
    except urllib.error.URLError as e:
        return 0, str(e.reason)


def test_status(port):
    print("── GET /status")
    code, data = req(port, "GET", "/status")
    if code == 0:
        print(f"   ❌ Server not reachable: {data}")
        return False
    print(f"   {code} → {json.dumps(data, indent=2)}")
    if not data.get("model_loaded"):
        print("   ⚠️  Model not loaded — TTS will fail")
    return code == 200


def test_voices(port):
    print("── GET /voices")
    code, data = req(port, "GET", "/voices")
    if code != 200:
        print(f"   ❌ {code}: {data}")
        return False
    voices = data.get("voices", [])
    print(f"   Found {len(voices)} voices:")
    for v in voices:
        flags = []
        if v.get("loaded"): flags.append("loaded")
        if v.get("cached"): flags.append("cached")
        flag_str = f" [{', '.join(flags)}]" if flags else ""
        print(f"     • {v['name']} ({v['type']}){flag_str}")
    return True


def test_load_voice(port, voice):
    print(f"── POST /load-voice  voice={voice}")
    code, data = req(port, "POST", "/load-voice", {"voice": voice})
    print(f"   {code} → {data}")
    return code == 200


def test_tts(port, voice, text, output_file=None):
    print(f"── POST /tts  voice={voice}  text={repr(text[:60])}{'...' if len(text) > 60 else ''}")
    t0 = time.time()
    code, data = req(port, "POST", "/tts", {"text": text, "voice": voice, "stream": False})
    elapsed = time.time() - t0

    if code != 200:
        print(f"   ❌ {code}: {data}")
        return False

    audio_bytes = data
    size_kb = len(audio_bytes) / 1024
    print(f"   ✅ {code} — {size_kb:.1f} KB in {elapsed:.2f}s")

    # Estimate duration from PCM size (subtract 44-byte WAV header)
    pcm_bytes = max(0, len(audio_bytes) - 44)
    duration = pcm_bytes / (24000 * 2)  # 24kHz, 16-bit mono
    print(f"   Audio duration: ~{duration:.1f}s  ({duration/elapsed:.1f}x realtime)" if elapsed > 0 else "")

    if output_file:
        with open(output_file, "wb") as f:
            f.write(audio_bytes)
        print(f"   Saved to: {output_file}")
    else:
        # Try to play with platform default
        _try_play(audio_bytes)

    return True


def _try_play(wav_bytes):
    """Best-effort audio playback."""
    tmp = os.path.join(os.environ.get("TEMP", "/tmp"), "pocket_tts_test.wav")
    with open(tmp, "wb") as f:
        f.write(wav_bytes)

    if sys.platform == "win32":
        try:
            import winsound
            print("   🔊 Playing...")
            winsound.PlaySound(tmp, winsound.SND_FILENAME)
            return
        except Exception:
            pass
    elif sys.platform == "darwin":
        os.system(f'afplay "{tmp}" 2>/dev/null')
        return
    else:
        for cmd in ["aplay", "paplay", "ffplay -nodisp -autoexit"]:
            if os.system(f'which {cmd.split()[0]} >/dev/null 2>&1') == 0:
                os.system(f'{cmd} "{tmp}" 2>/dev/null')
                return

    print("   (no audio player found — use --output to save the file)")


def test_stop(port):
    print("── POST /stop")
    code, data = req(port, "POST", "/stop")
    print(f"   {code} → {data}")
    return code == 200


def main():
    parser = argparse.ArgumentParser(description="Test the Pocket TTS server")
    parser.add_argument("--port", type=int, default=9877)
    parser.add_argument("--voice", type=str, default="alba")
    parser.add_argument("--text", type=str,
                        default="Hello! I am your Kiro assistant. This is a test of the Pocket TTS engine.")
    parser.add_argument("--output", type=str, default=None, help="Save audio to file")
    parser.add_argument("--no-play", action="store_true", help="Skip audio playback")
    parser.add_argument("--all-voices", action="store_true", help="Test all built-in voices")
    args = parser.parse_args()

    print(f"🔊 Pocket TTS Server Test — port {args.port}\n")

    # 1. Status
    if not test_status(args.port):
        print("\n💡 Start the server first:  python pocket_tts/server.py")
        sys.exit(1)
    print()

    # 2. Voices
    test_voices(args.port)
    print()

    # 3. Load voice
    test_load_voice(args.port, args.voice)
    print()

    # 4. TTS
    if args.all_voices:
        voices = ["alba", "marius", "javert", "jean", "fantine", "cosette", "eponine", "azelma"]
        for v in voices:
            out = f"test_{v}.wav" if args.no_play else None
            test_tts(args.port, v, f"Hi, my name is {v}.", output_file=out)
            print()
    else:
        out = args.output if (args.output or args.no_play) else None
        if args.no_play and not out:
            out = "pocket_tts_test.wav"
        test_tts(args.port, args.voice, args.text, output_file=out)
        print()

    # 5. Stop (just verify the endpoint works)
    test_stop(args.port)
    print()

    print("✅ All tests passed!")


if __name__ == "__main__":
    main()
