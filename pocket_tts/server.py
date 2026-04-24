"""
Pocket TTS streaming server for Kage.

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
import logging
import os
import re
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
DEBUG = False  # set via --debug flag

BUILTIN_VOICES = ["alba", "marius", "javert", "jean", "fantine", "cosette", "eponine", "azelma"]
SAMPLE_RATE = 24000  # pocket-tts default

logger = logging.getLogger("pocket-tts")


def setup_logging(debug=False):
    """Configure logging. In debug mode, writes verbose logs to a file (append mode, pruned on start)."""
    logger.setLevel(logging.DEBUG if debug else logging.INFO)
    logger.propagate = False  # Prevent root logger from writing to broken stdout/stderr

    if debug:
        log_dir = get_data_dir()
        os.makedirs(log_dir, exist_ok=True)
        log_path = os.path.join(log_dir, "pocket_tts_server.log")

        # Prune lines older than 24 hours
        _prune_old_log_entries(log_path, max_age_hours=24)

        # File handler only — stdout StreamHandler causes OSError on Windows
        # when parent process captures stdout via pipes (Python 3.14 issue)
        file_handler = logging.FileHandler(log_path, mode="a", encoding="utf-8")
        file_handler.setLevel(logging.DEBUG)
        file_handler.setFormatter(logging.Formatter(
            "%(asctime)s [%(levelname)s] %(message)s", datefmt="%Y-%m-%d %H:%M:%S"
        ))
        logger.addHandler(file_handler)
        logger.debug("--- Server starting (log: %s) ---", log_path)


def _prune_old_log_entries(log_path, max_age_hours=24):
    """Remove log lines older than max_age_hours. Keeps recent entries."""
    if not os.path.isfile(log_path):
        return
    try:
        cutoff = time.time() - (max_age_hours * 3600)
        kept = []
        with open(log_path, "r", encoding="utf-8", errors="replace") as f:
            for line in f:
                # Parse timestamp from start of line: "YYYY-MM-DD HH:MM:SS ..."
                try:
                    ts_str = line[:19]
                    ts = time.mktime(time.strptime(ts_str, "%Y-%m-%d %H:%M:%S"))
                    if ts >= cutoff:
                        kept.append(line)
                except (ValueError, OverflowError):
                    kept.append(line)  # Keep unparseable lines
        with open(log_path, "w", encoding="utf-8") as f:
            f.writelines(kept)
    except Exception as e:
        # Non-fatal — just skip pruning. Log to stderr (not logger, since logger
        # may not be configured yet at the point this runs).
        print(f"[pocket_tts] log prune failed: {e}", file=sys.stderr)


def dbg(msg):
    """Log a debug message to file only."""
    logger.debug(msg)


def get_data_dir():
    """Return the kage pocket-tts data directory."""
    if sys.platform == "win32":
        base = os.environ.get("LOCALAPPDATA", os.path.expanduser("~"))
    elif sys.platform == "darwin":
        base = os.path.join(os.path.expanduser("~"), "Library", "Application Support")
    else:
        base = os.environ.get("XDG_DATA_HOME", os.path.join(os.path.expanduser("~"), ".local", "share"))
    path = os.path.join(base, "kage", "pocket-tts")
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
        logger.info("Loading model...")
        t0 = time.time()
        # temp and eos_threshold can be tuned:
        #   temp: 0.5 = more consistent, 0.7 = default, 0.9 = more expressive
        #   eos_threshold: -4.0 default, lower = less likely to stop early
        temp = float(os.environ.get("POCKET_TTS_TEMP", "0.7"))
        eos_threshold = float(os.environ.get("POCKET_TTS_EOS_THRESHOLD", "-4.0"))
        model = TTSModel.load_model(temp=temp, eos_threshold=eos_threshold)
        elapsed = time.time() - t0
        logger.info("Model loaded in %.1fs (temp=%s, eos=%s)", elapsed, temp, eos_threshold)
        return True
    except ImportError:
        logger.error("pocket-tts not installed. Run: pip install pocket-tts")
        return False
    except Exception as e:
        logger.error("Error loading model: %s", e)
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
            logger.info("Auto-exported '%s' to safetensors for fast loading", voice_name)
    except Exception as e:
        # Non-fatal — just means next load will be slower
        logger.warning("Failed to auto-export '%s': %s", voice_name, e)


def _is_safe_voice_name(name):
    """Local voice names must be a simple identifier. Rejects anything with
    path separators, `..`, control chars, or shell metacharacters so that
    `os.path.join(dir, name)` can't escape the voices directory."""
    if not isinstance(name, str) or not name:
        return False
    if len(name) > 128:
        return False
    # Allow letters, digits, underscore, dash, dot (for e.g. alpha-variants).
    # Explicitly disallow leading dot (would hide files) and `..`.
    if name.startswith(".") or ".." in name:
        return False
    return re.match(r"^[A-Za-z0-9][A-Za-z0-9_.\-]*$", name) is not None


def get_voice_state(voice_name):
    """Get or create a cached voice state for the given voice.

    Load priority:
      1. In-memory cache (instant)
      2. Auto-exported safetensors in cache dir (fast)
      3. User safetensors in voices dir (fast)
      4. User .wav in voices dir (slow, then auto-export)
      5. Built-in voice name (slow, then auto-export)

    URL loading (hf://, http(s)://) is disabled by default; set the
    POCKET_TTS_ALLOW_URL_VOICES=1 environment variable to re-enable it.
    """
    if voice_name in voice_states:
        return voice_states[voice_name]

    if model is None:
        return None

    # Classify the request up-front.
    is_url = isinstance(voice_name, str) and (
        voice_name.startswith("hf://")
        or voice_name.startswith("http://")
        or voice_name.startswith("https://")
    )

    if is_url:
        if os.environ.get("POCKET_TTS_ALLOW_URL_VOICES", "0") != "1":
            logger.warning("URL voice loading is disabled: %s", voice_name)
            return None
    else:
        if not _is_safe_voice_name(voice_name):
            logger.warning("Rejected unsafe voice name: %r", voice_name)
            return None

    try:
        state = None
        needs_export = False

        if not is_url:
            # 1. Check auto-exported cache
            cache_path = os.path.join(get_cache_dir(), f"{voice_name}.safetensors")
            if os.path.isfile(cache_path):
                logger.info("Loading voice from cache: %s", voice_name)
                state = model.get_state_for_audio_prompt(cache_path)

            # 2. Check user safetensors
            if state is None:
                user_st = os.path.join(get_custom_voices_dir(), f"{voice_name}.safetensors")
                if os.path.isfile(user_st):
                    logger.info("Loading voice from user safetensors: %s", voice_name)
                    state = model.get_state_for_audio_prompt(user_st)

            # 3. Check user .wav
            if state is None:
                wav_path = os.path.join(get_custom_voices_dir(), f"{voice_name}.wav")
                if os.path.isfile(wav_path):
                    logger.info("Loading voice from wav: %s", voice_name)
                    state = model.get_state_for_audio_prompt(wav_path)
                    needs_export = True

        # 4. HuggingFace or HTTP URL — gated above
        if state is None and is_url:
            logger.info("Loading voice from URL: %s", voice_name)
            state = model.get_state_for_audio_prompt(voice_name)
            needs_export = True

        # 5. Built-in voice
        if state is None and not is_url and voice_name in BUILTIN_VOICES:
            logger.info("Loading built-in voice: %s", voice_name)
            state = model.get_state_for_audio_prompt(voice_name)
            needs_export = True

        if state is None:
            logger.warning("Unknown voice: %s", voice_name)
            return None

        voice_states[voice_name] = state
        logger.info("Voice '%s' ready", voice_name)

        # Auto-export to safetensors for fast loading next time
        if needs_export:
            threading.Thread(
                target=_auto_export_safetensors,
                args=(voice_name, state),
                daemon=True,
            ).start()

        return state
    except Exception as e:
        logger.error("Error loading voice '%s': %s", voice_name, e)
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
        # In debug mode, log all requests; otherwise only non-200
        if DEBUG:
            logger.debug("HTTP %s", format % args)
        elif len(args) >= 2 and str(args[1]) != "200":
            logger.info("HTTP %s", format % args)

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
        dbg(f"OPTIONS {self.path} from {self.client_address}")
        self.send_response(204)
        self.send_header("Access-Control-Allow-Origin", "*")
        self.send_header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
        self.send_header("Access-Control-Allow-Headers", "Content-Type")
        self.end_headers()

    def do_GET(self):
        dbg(f"GET {self.path} from {self.client_address}")
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
        dbg(f"POST {self.path} from {self.client_address}")
        try:
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
        except (BrokenPipeError, ConnectionAbortedError, ConnectionResetError):
            pass
        except Exception as e:
            logger.error("Unhandled POST error on %s: %s", self.path, e)
            import traceback
            traceback.print_exc()
            try:
                self._json_response(500, {"error": str(e)})
            except Exception:
                pass


    def _handle_tts(self):
        """Generate speech and stream audio chunks back as they're generated."""
        if model is None:
            logger.warning("TTS request rejected — model not loaded")
            self._json_response(503, {"error": "Model not loaded"})
            return

        try:
            body = self._read_json_body()
        except Exception as e:
            dbg(f"TTS request — invalid JSON: {e}")
            self._json_response(400, {"error": f"Invalid JSON: {e}"})
            return

        text = body.get("text", "").strip()
        voice = body.get("voice", "alba")
        # Allow caller to request non-streaming (for test script compatibility)
        stream = body.get("stream", True)

        dbg(f"TTS request — text='{text[:50]}...' voice={voice} stream={stream}")

        if not text:
            self._json_response(400, {"error": "No text provided"})
            return

        if not model_lock.acquire(timeout=0.1):
            dbg("TTS request rejected — generation already in progress")
            self._json_response(429, {"error": "Generation already in progress"})
            return

        current_generation.set()
        cancel_flag.clear()

        try:
            voice_state = get_voice_state(voice)
            if voice_state is None:
                dbg(f"TTS — voice '{voice}' not available")
                self._json_response(400, {"error": f"Voice '{voice}' not available"})
                return

            dbg(f"TTS — voice '{voice}' loaded, starting generation")
            import torch
            import numpy as np

            if stream and hasattr(model, 'generate_audio_stream'):
                # Streaming mode: send chunks as they're generated
                # Don't use Transfer-Encoding: chunked — write raw PCM directly.
                # Some webview fetch implementations don't decode chunked encoding.
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("X-Sample-Rate", str(SAMPLE_RATE))
                self.send_header("X-Channels", "1")
                self.send_header("X-Bits-Per-Sample", "16")
                self.send_header("Access-Control-Allow-Origin", "*")
                self.send_header("Cache-Control", "no-store")
                self.end_headers()

                for chunk in model.generate_audio_stream(voice_state, text, frames_after_eos=2):
                    if cancel_flag.is_set():
                        dbg("TTS — cancelled via cancel_flag")
                        break

                    if isinstance(chunk, torch.Tensor):
                        chunk_np = chunk.numpy()
                    else:
                        chunk_np = np.array(chunk)

                    chunk_np = np.clip(chunk_np, -1.0, 1.0)
                    pcm = (chunk_np * 32767).astype(np.int16)
                    pcm_bytes = pcm.tobytes()

                    try:
                        self.wfile.write(pcm_bytes)
                        self.wfile.flush()
                    except (BrokenPipeError, ConnectionAbortedError, ConnectionResetError, OSError):
                        dbg("TTS — client disconnected, cancelling generation")
                        cancel_flag.set()
                        break
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
            logger.error("TTS error: %s", e)
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
            logger.info("Loading voice on demand: %s", voice)
            state = get_voice_state(voice)
            if state is None:
                self._json_response(400, {"error": f"Failed to load voice '{voice}'"})
            else:
                logger.info("Voice '%s' loaded successfully", voice)
                self._json_response(200, {"status": "loaded", "voice": voice})
        except Exception as e:
            logger.error("Error loading voice: %s", e)
            import traceback
            traceback.print_exc()
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

    # Note: UTF-8 encoding for stdout/stderr is set via PYTHONIOENCODING env var
    # in the Rust launcher to avoid cp1252 encoding crashes on Windows.

    parser = argparse.ArgumentParser(description="Pocket TTS server for Kage")
    parser.add_argument("--port", type=int, default=9877, help="Port to listen on")
    parser.add_argument("--voice", type=str, default="alba", help="Default voice to pre-load")
    parser.add_argument("--no-preload", action="store_true", help="Don't pre-load model at startup")
    parser.add_argument("--temp", type=float, default=None, help="Sampling temperature (0.5=consistent, 0.7=default, 0.9=expressive)")
    parser.add_argument("--eos-threshold", type=float, default=None, help="End-of-sequence threshold (default: -4.0)")
    parser.add_argument("--debug", action="store_true", help="Enable verbose debug logging")
    args = parser.parse_args()

    global DEBUG
    DEBUG = args.debug
    setup_logging(debug=DEBUG)

    # Pass temp/eos via env vars so load_model() picks them up
    if args.temp is not None:
        os.environ["POCKET_TTS_TEMP"] = str(args.temp)
    if args.eos_threshold is not None:
        os.environ["POCKET_TTS_EOS_THRESHOLD"] = str(args.eos_threshold)

    data_dir = get_data_dir()
    logger.info("Data directory: %s", data_dir)
    logger.info("Custom voices: %s", get_custom_voices_dir())
    logger.info("Cache: %s", get_cache_dir())

    if not args.no_preload:
        if not load_model():
            logger.warning("Model failed to load. Server will start but TTS won't work.")
            logger.warning("Install with: pip install pocket-tts")
        else:
            # Pre-load default voice (will auto-export to safetensors)
            get_voice_state(args.voice)

    server = ThreadedHTTPServer(("127.0.0.1", args.port), TTSHandler)
    logger.info("Server listening on http://127.0.0.1:%d", args.port)
    # Signal to parent process that we're ready (must be raw print, not logger)
    print("POCKET_TTS_READY", flush=True)

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        logger.info("Shutting down...")
    finally:
        server.server_close()


if __name__ == "__main__":
    main()
