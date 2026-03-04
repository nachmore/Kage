"""
Pocket TTS streaming server for Kiro Assistant.

Wraps kyutai-labs/pocket-tts as an HTTP server that accepts text
and streams back WAV audio chunks for low-latency playback.

Endpoints:
  POST /tts          — Generate speech from JSON { "text": "...", "voice": "alba" }
                       Returns streaming WAV audio (chunked transfer).
  GET  /voices       — List available voices (built-in + downloaded).
  GET  /status       — Health check / model loaded status.
  POST /stop         — Stop any in-progress generation.
  POST /export-voice — Export a .wav to .safetensors for fast loading.
  POST /load-voice   — Pre-load a voice into cache.

Usage:
  python server.py [--port 9877] [--voice alba]
"""

import argparse
import json
import os
import struct
import sys
import threading
import time
from http.server import HTTPServer, BaseHTTPRequestHandler

# ---------------------------------------------------------------------------
# Globals
# ---------------------------------------------------------------------------
model = None
model_lock = threading.Lock()
voice_states = {}  # voice_name -> cached voice state
current_generation = threading.Event()  # set = generation in progress
cancel_flag = threading.Event()
data_dir = None  # resolved at startup

BUILTIN_VOICES = ["alba", "marius", "javert", "jean", "fantine", "cosette", "eponine", "azelma"]
SAMPLE_RATE = 24000  # pocket-tts default


def get_data_dir():
    """Return the Kiro pocket-tts data directory."""
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA", os.path.expanduser("~"))
    elif sys.platform == "darwin":
        base = os.path.join(os.path.expanduser("~"), "Library", "Application Support")
    else:
        base = os.environ.get("XDG_DATA_HOME", os.path.join(os.path.expanduser("~"), ".local", "share"))
    path = os.path.join(base, "kiro-assistant", "pocket-tts")
    os.makedirs(path, exist_ok=True)
    return path


def get_custom_voices_dir():
    """Return directory for user-downloaded voice files."""
    path = os.path.join(data_dir, "voices")
    os.makedirs(path, exist_ok=True)
    return path


def get_cache_dir():
    """Return directory for cached safetensors voice exports."""
    path = os.path.join(data_dir, "cache")
    os.makedirs(path, exist_ok=True)
    return path


def load_model():
    """Load the pocket-tts model (slow — do once at startup)."""
    global model
    try:
        from pocket_tts import TTSModel
        print("[pocket-tts] Loading model...", flush=True)
        t0 = time.time()
        # temp and eos_threshold can be tuned:
        #   temp: 0.5 = more consistent, 0.7 = default, 0.9 = more expressive
        #   eos_threshold: -4.0 default, lower = less likely to stop early
        temp = float(os.environ.get("POCKET_TTS_TEMP", "0.7"))
        eos_threshold = float(os.environ.get("POCKET_TTS_EOS_THRESHOLD", "-4.0"))
        model = TTSModel.load_model(temp=temp, eos_threshold=eos_threshold)
        elapsed = time.time() - t0
        print(f"[pocket-tts] Model loaded in {elapsed:.1f}s (temp={temp}, eos={eos_threshold})", flush=True)
        return True
    except ImportError:
        print("[pocket-tts] ERROR: pocket-tts not installed. Run: pip install pocket-tts", flush=True)
        return False
    except Exception as e:
        print(f"[pocket-tts] ERROR loading model: {e}", flush=True)
        return False


def _auto_export_safetensors(voice_name, state):
    """Auto-export a voice state to safetensors for fast loading next time."""
    try:
        from pocket_tts import export_model_state
        # Use a filesystem-safe name for the cache key
        safe_name = voice_name.replace("/", "_").replace(":", "_").replace("?", "_")
        cache_path = os.path.join(get_cache_dir(), f"{safe_name}.safetensors")
        if not os.path.isfile(cache_path):
            export_model_state(state, cache_path)
            print(f"[pocket-tts] Auto-exported '{voice_name}' to safetensors for fast loading", flush=True)
    except Exception as e:
        # Non-fatal — just means next load will be slower
        print(f"[pocket-tts] Warning: failed to auto-export '{voice_name}': {e}", flush=True)


def get_voice_state(voice_name):
    """Get or create a cached voice state for the given voice.

    Load priority:
      1. In-memory cache (instant)
      2. Auto-exported safetensors in cache dir (fast)
      3. User safetensors in voices dir (fast)
      4. User .wav in voices dir (slow, then auto-export)
      5. Built-in voice name (slow, then auto-export)
    """
    if voice_name in voice_states:
        return voice_states[voice_name]

    if model is None:
        return None

    try:
        state = None
        needs_export = False

        # 1. Check auto-exported cache
        cache_path = os.path.join(get_cache_dir(), f"{voice_name}.safetensors")
        if os.path.isfile(cache_path):
            print(f"[pocket-tts] Loading voice from cache: {voice_name}", flush=True)
            state = model.get_state_for_audio_prompt(cache_path)

        # 2. Check user safetensors
        if state is None:
            user_st = os.path.join(get_custom_voices_dir(), f"{voice_name}.safetensors")
            if os.path.isfile(user_st):
                print(f"[pocket-tts] Loading voice from user safetensors: {voice_name}", flush=True)
                state = model.get_state_for_audio_prompt(user_st)

        # 3. Check user .wav
        if state is None:
            wav_path = os.path.join(get_custom_voices_dir(), f"{voice_name}.wav")
            if os.path.isfile(wav_path):
                print(f"[pocket-tts] Loading voice from wav: {voice_name}", flush=True)
                state = model.get_state_for_audio_prompt(wav_path)
                needs_export = True

        # 4. HuggingFace or HTTP URL
        if state is None and (voice_name.startswith("hf://") or voice_name.startswith("http")):
            print(f"[pocket-tts] Loading voice from URL: {voice_name}", flush=True)
            state = model.get_state_for_audio_prompt(voice_name)
            needs_export = True

        # 5. Built-in voice
        if state is None and voice_name in BUILTIN_VOICES:
            print(f"[pocket-tts] Loading built-in voice: {voice_name}", flush=True)
            state = model.get_state_for_audio_prompt(voice_name)
            needs_export = True

        if state is None:
            print(f"[pocket-tts] Unknown voice: {voice_name}", flush=True)
            return None

        voice_states[voice_name] = state
        print(f"[pocket-tts] Voice '{voice_name}' ready", flush=True)

        # Auto-export to safetensors for fast loading next time
        if needs_export:
            threading.Thread(
                target=_auto_export_safetensors,
                args=(voice_name, state),
                daemon=True,
            ).start()

        return state
    except Exception as e:
        print(f"[pocket-tts] ERROR loading voice '{voice_name}': {e}", flush=True)
        return None


def list_available_voices():
    """List all available voices (built-in + custom)."""
    voices = []
    for v in BUILTIN_VOICES:
        # Check if we have a cached safetensors for this built-in
        cached = os.path.isfile(os.path.join(get_cache_dir(), f"{v}.safetensors"))
        voices.append({
            "name": v,
            "type": "builtin",
            "loaded": v in voice_states,
            "cached": cached,
        })

    custom_dir = get_custom_voices_dir()
    seen = {v["name"] for v in voices}
    if os.path.isdir(custom_dir):
        for f in sorted(os.listdir(custom_dir)):
            name, ext = os.path.splitext(f)
            if ext.lower() in (".wav", ".safetensors", ".mp3"):
                if name not in seen:
                    seen.add(name)
                    cached = os.path.isfile(os.path.join(get_cache_dir(), f"{name}.safetensors"))
                    voices.append({
                        "name": name,
                        "type": "custom",
                        "loaded": name in voice_states,
                        "cached": cached,
                    })
    return voices


def wav_header(sample_rate, num_channels=1, bits_per_sample=16, data_size=0):
    """Create a WAV header with the actual data size."""
    byte_rate = sample_rate * num_channels * bits_per_sample // 8
    block_align = num_channels * bits_per_sample // 8
    header = struct.pack('<4sI4s', b'RIFF', data_size + 36, b'WAVE')
    header += struct.pack('<4sIHHIIHH', b'fmt ', 16, 1, num_channels,
                          sample_rate, byte_rate, block_align, bits_per_sample)
    header += struct.pack('<4sI', b'data', data_size)
    return header


class TTSHandler(BaseHTTPRequestHandler):
    """HTTP request handler for the TTS server."""

    def log_message(self, format, *args):
        if len(args) >= 2 and str(args[1]) == "200":
            return
        print(f"[pocket-tts] {format % args}", flush=True)

    def _json_response(self, code, data):
        body = json.dumps(data).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Access-Control-Allow-Origin", "*")
        self.end_headers()
        self.wfile.write(body)

    def _read_json_body(self):
        length = int(self.headers.get("Content-Length", 0))
        if length == 0:
            return {}
        return json.loads(self.rfile.read(length))

    def do_OPTIONS(self):
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.end_headers()

    def do_GET(self):
        if self.path == "/status":
            self._json_response(200, {
                "status": "ok",
                "model_loaded": model is not None,
                "generating": current_generation.is_set(),
                "voices_loaded": list(voice_states.keys()),
            })
        elif self.path == "/voices":
            self._json_response(200, {"voices": list_available_voices()})
        else:
            self._json_response(404, {"error": "not found"})

    def do_POST(self):
        if self.path == "/tts":
            self._handle_tts()
        elif self.path == "/stop":
            cancel_flag.set()
            self._json_response(200, {"status": "cancelled"})
        elif self.path == "/export-voice":
            self._handle_export_voice()
        elif self.path == "/load-voice":
            self._handle_load_voice()
        else:
            self._json_response(404, {"error": "not found"})


    def _handle_tts(self):
        """Generate speech and stream audio chunks back as they're generated."""
        if model is None:
            self._json_response(503, {"error": "Model not loaded"})
            return

        try:
            body = self._read_json_body()
        except Exception as e:
            self._json_response(400, {"error": f"Invalid JSON: {e}"})
            return

        text = body.get("text", "").strip()
        voice = body.get("voice", "alba")
        # Allow caller to request non-streaming (for test script compatibility)
        stream = body.get("stream", True)

        if not text:
            self._json_response(400, {"error": "No text provided"})
            return

        if not model_lock.acquire(timeout=0.1):
            self._json_response(429, {"error": "Generation already in progress"})
            return

        current_generation.set()
        cancel_flag.clear()

        try:
            voice_state = get_voice_state(voice)
            if voice_state is None:
                self._json_response(400, {"error": f"Voice '{voice}' not available"})
                return

            import torch
            import numpy as np

            if stream and hasattr(model, 'generate_audio_stream'):
                # Streaming mode: send chunks as they're generated
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("X-Sample-Rate", str(SAMPLE_RATE))
                self.send_header("X-Channels", "1")
                self.send_header("X-Bits-Per-Sample", "16")
                self.send_header("Transfer-Encoding", "chunked")
                self.send_header("Access-Control-Allow-Origin", "*")
                self.send_header("Cache-Control", "no-store")
                self.end_headers()

                for chunk in model.generate_audio_stream(voice_state, text, frames_after_eos=2):
                    if cancel_flag.is_set():
                        break

                    if isinstance(chunk, torch.Tensor):
                        chunk_np = chunk.numpy()
                    else:
                        chunk_np = np.array(chunk)

                    chunk_np = np.clip(chunk_np, -1.0, 1.0)
                    pcm = (chunk_np * 32767).astype(np.int16)
                    pcm_bytes = pcm.tobytes()

                    # Write chunked transfer encoding frame
                    chunk_header = f"{len(pcm_bytes):X}\r\n".encode()
                    self.wfile.write(chunk_header)
                    self.wfile.write(pcm_bytes)
                    self.wfile.write(b"\r\n")
                    self.wfile.flush()

                # Write final zero-length chunk to signal end
                self.wfile.write(b"0\r\n\r\n")
                self.wfile.flush()
            else:
                # Non-streaming fallback: generate full audio, return as WAV
                audio = model.generate_audio(voice_state, text, frames_after_eos=2)

                if cancel_flag.is_set():
                    self._json_response(499, {"error": "Cancelled"})
                    return

                if isinstance(audio, torch.Tensor):
                    audio_np = audio.numpy()
                else:
                    audio_np = np.array(audio)

                audio_np = np.clip(audio_np, -1.0, 1.0)
                pcm = (audio_np * 32767).astype(np.int16)
                pcm_bytes = pcm.tobytes()

                header = wav_header(SAMPLE_RATE, data_size=len(pcm_bytes))
                wav_data = header + pcm_bytes

                self.send_response(200)
                self.send_header("Content-Type", "audio/wav")
                self.send_header("Content-Length", str(len(wav_data)))
                self.send_header("Access-Control-Allow-Origin", "*")
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(wav_data)
                self.wfile.flush()

        except (BrokenPipeError, ConnectionAbortedError, ConnectionResetError):
            pass
        except Exception as e:
            print(f"[pocket-tts] TTS error: {e}", flush=True)
        finally:
            current_generation.clear()
            model_lock.release()

    def _handle_load_voice(self):
        """Pre-load a voice into cache."""
        try:
            body = self._read_json_body()
            voice = body.get("voice", "")
            if not voice:
                self._json_response(400, {"error": "No voice specified"})
                return
            state = get_voice_state(voice)
            if state is None:
                self._json_response(400, {"error": f"Failed to load voice '{voice}'"})
            else:
                self._json_response(200, {"status": "loaded", "voice": voice})
        except Exception as e:
            self._json_response(500, {"error": str(e)})

    def _handle_export_voice(self):
        """Export a .wav voice to .safetensors for fast loading."""
        if model is None:
            self._json_response(503, {"error": "Model not loaded"})
            return
        try:
            body = self._read_json_body()
            wav_path = body.get("wav_path", "")
            output_name = body.get("output_name", "")
            if not wav_path or not output_name:
                self._json_response(400, {"error": "wav_path and output_name required"})
                return

            from pocket_tts import export_model_state
            state = model.get_state_for_audio_prompt(wav_path)
            out_path = os.path.join(get_custom_voices_dir(), f"{output_name}.safetensors")
            export_model_state(state, out_path)
            voice_states[output_name] = state
            self._json_response(200, {"status": "exported", "path": out_path})
        except Exception as e:
            self._json_response(500, {"error": str(e)})


class ThreadedHTTPServer(HTTPServer):
    """Threaded HTTP server that handles each request in a new thread."""
    allow_reuse_address = True
    daemon_threads = True

    def handle_error(self, request, client_address):
        exc_type = sys.exc_info()[0]
        if exc_type in (BrokenPipeError, ConnectionAbortedError, ConnectionResetError, OSError):
            return
        super().handle_error(request, client_address)


def main():
    global data_dir

    parser = argparse.ArgumentParser(description="Pocket TTS server for Kiro Assistant")
    parser.add_argument("--port", type=int, default=9877, help="Port to listen on")
    parser.add_argument("--voice", type=str, default="alba", help="Default voice to pre-load")
    parser.add_argument("--no-preload", action="store_true", help="Don't pre-load model at startup")
    parser.add_argument("--temp", type=float, default=None, help="Sampling temperature (0.5=consistent, 0.7=default, 0.9=expressive)")
    parser.add_argument("--eos-threshold", type=float, default=None, help="End-of-sequence threshold (default: -4.0)")
    args = parser.parse_args()

    # Pass temp/eos via env vars so load_model() picks them up
    if args.temp is not None:
        os.environ["POCKET_TTS_TEMP"] = str(args.temp)
    if args.eos_threshold is not None:
        os.environ["POCKET_TTS_EOS_THRESHOLD"] = str(args.eos_threshold)

    data_dir = get_data_dir()
    print(f"[pocket-tts] Data directory: {data_dir}", flush=True)
    print(f"[pocket-tts] Custom voices: {get_custom_voices_dir()}", flush=True)
    print(f"[pocket-tts] Cache: {get_cache_dir()}", flush=True)

    if not args.no_preload:
        if not load_model():
            print("[pocket-tts] WARNING: Model failed to load. Server will start but TTS won't work.", flush=True)
            print("[pocket-tts] Install with: pip install pocket-tts", flush=True)
        else:
            # Pre-load default voice (will auto-export to safetensors)
            get_voice_state(args.voice)

    server = ThreadedHTTPServer(("127.0.0.1", args.port), TTSHandler)
    print(f"[pocket-tts] Server listening on http://127.0.0.1:{args.port}", flush=True)
    # Signal to parent process that we're ready
    print("POCKET_TTS_READY", flush=True)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("[pocket-tts] Shutting down...", flush=True)
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
