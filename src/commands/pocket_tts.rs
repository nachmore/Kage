use crate::state::AppState;
use log::{info, warn};
use serde::{Deserialize, Serialize};
use std::io::BufRead;
use std::process::{Command, Stdio};
use tauri::{Emitter, State};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PocketTtsStatus {
    pub server_running: bool,
    pub installed: bool,
    pub python_found: bool,
    pub python_path: Option<String>,
    pub port: u16,
}

/// Platform-specific: hide console window on Windows
#[cfg(target_os = "windows")]
pub fn configure_no_window(cmd: &mut Command) -> &mut Command {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x08000000) // CREATE_NO_WINDOW
}

#[cfg(not(target_os = "windows"))]
pub fn configure_no_window(cmd: &mut Command) -> &mut Command {
    cmd
}

/// Find a working Python 3 executable
fn find_python() -> Option<String> {
    let candidates = if cfg!(target_os = "windows") {
        vec!["python", "python3"]
    } else {
        vec!["python3", "python"]
    };

    for candidate in &candidates {
        let mut cmd = Command::new(candidate);
        cmd.arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        configure_no_window(&mut cmd);

        if let Ok(child) = cmd.spawn() {
            if let Ok(output) = child.wait_with_output() {
                let version_str = String::from_utf8_lossy(&output.stdout).to_string()
                    + &String::from_utf8_lossy(&output.stderr);
                if version_str.contains("Python 3.") {
                    return Some(candidate.to_string());
                }
            }
        }
    }
    None
}

/// Check if pocket-tts pip package is installed
fn check_pocket_tts_installed(python: &str) -> bool {
    let mut cmd = Command::new(python);
    cmd.args(["-c", "import pocket_tts; print('ok')"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_no_window(&mut cmd);

    if let Ok(child) = cmd.spawn() {
        if let Ok(output) = child.wait_with_output() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            return stdout.trim() == "ok";
        }
    }
    false
}

/// Check if the pocket-tts server is responding
async fn check_server_running(port: u16) -> bool {
    let url = format!("http://127.0.0.1:{}/status", port);
    match reqwest::get(&url).await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Get the path to the pocket_tts/server.py script.
///
/// Resolution order:
///   1. `pocket_tts/server.py` relative to CWD (dev mode)
///   2. Next to the executable: `<exe_dir>/pocket_tts/server.py` (bundled install)
///   3. One level up from exe (some installer layouts): `<exe_dir>/../pocket_tts/server.py`
pub fn get_server_script_path() -> std::path::PathBuf {
    // Dev mode: relative to project root
    let dev_path = std::path::PathBuf::from("pocket_tts/server.py");
    if dev_path.exists() {
        return dev_path;
    }

    // Production: next to the executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let bundled = exe_dir.join("pocket_tts").join("server.py");
            if bundled.exists() {
                return bundled;
            }
            // One level up (some layouts put resources in parent)
            if let Some(parent) = exe_dir.parent() {
                let up_one = parent.join("pocket_tts").join("server.py");
                if up_one.exists() {
                    return up_one;
                }
            }
        }
    }

    // Fallback — return the dev path and let the caller handle the error
    dev_path
}


#[tauri::command]
pub async fn pocket_tts_status(state: State<'_, AppState>) -> Result<PocketTtsStatus, String> {
    let (port, python_path) = {
        let config = state.config.lock().unwrap();
        let pp = config.pocket_tts.python_path.clone().or_else(|| find_python());
        (config.pocket_tts.port, pp)
    };

    let python_found = python_path.is_some();
    let installed = if let Some(ref py) = python_path {
        check_pocket_tts_installed(py)
    } else {
        false
    };

    let server_running = check_server_running(port).await;

    Ok(PocketTtsStatus {
        server_running,
        installed,
        python_found,
        python_path,
        port,
    })
}

#[tauri::command]
pub async fn pocket_tts_install(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let config = state.config.lock().unwrap();
    let python = config
        .pocket_tts
        .python_path
        .clone()
        .or_else(|| find_python())
        .ok_or_else(|| "Python 3 not found. Please install Python 3.10+ first.".to_string())?;
    drop(config);

    // Check if an install is already running
    {
        let proc = state.pocket_tts_install_process.lock().unwrap();
        if proc.is_some() {
            return Err("Installation already in progress".to_string());
        }
    }

    info!("Installing pocket-tts via pip...");

    let mut cmd = Command::new(&python);
    cmd.args(["-m", "pip", "install", "pocket-tts"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_no_window(&mut cmd);

    let mut child = cmd.spawn().map_err(|e| format!("Failed to run pip: {}", e))?;

    // Take stdout and stderr for streaming
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    // Store the child process so it can be cancelled
    {
        let mut proc = state.pocket_tts_install_process.lock().unwrap();
        *proc = Some(child);
    }

    let app_handle = app.clone();
    let install_proc = state.pocket_tts_install_process.clone();
    let python_for_config = python.clone();

    // Spawn a thread to read output and emit events
    std::thread::spawn(move || {
        // Read stdout in a thread
        let app_for_stdout = app_handle.clone();
        let stdout_thread = stdout.map(|out| {
            let app = app_for_stdout;
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(out);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let _ = app.emit("pocket_tts_install_output", &line);
                    }
                }
            })
        });

        // Read stderr in a thread
        let app_for_stderr = app_handle.clone();
        let stderr_thread = stderr.map(|err| {
            let app = app_for_stderr;
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(err);
                for line in reader.lines() {
                    if let Ok(line) = line {
                        let _ = app.emit("pocket_tts_install_output", &line);
                    }
                }
            })
        });

        // Wait for output threads to finish
        if let Some(t) = stdout_thread { let _ = t.join(); }
        if let Some(t) = stderr_thread { let _ = t.join(); }

        // Wait for the process to exit
        let exit_status = {
            let mut proc = install_proc.lock().unwrap();
            if let Some(ref mut child) = *proc {
                child.wait().ok()
            } else {
                // Process was cancelled / taken
                None
            }
        };

        // Clear the install process
        {
            let mut proc = install_proc.lock().unwrap();
            *proc = None;
        }

        match exit_status {
            Some(status) if status.success() => {
                info!("pocket-tts installed successfully");
                let _ = app_handle.emit("pocket_tts_install_done", serde_json::json!({
                    "success": true,
                    "message": "pocket-tts installed successfully",
                    "python_path": python_for_config,
                }));
            }
            Some(_status) => {
                let _ = app_handle.emit("pocket_tts_install_done", serde_json::json!({
                    "success": false,
                    "message": "Installation failed (pip returned non-zero exit code)",
                }));
            }
            None => {
                // Process was cancelled
                let _ = app_handle.emit("pocket_tts_install_done", serde_json::json!({
                    "success": false,
                    "message": "Installation cancelled",
                }));
            }
        }
    });

    Ok("Installation started".to_string())
}

#[tauri::command]
pub async fn pocket_tts_cancel_install(state: State<'_, AppState>) -> Result<String, String> {
    let mut proc = state.pocket_tts_install_process.lock().unwrap();
    if let Some(mut child) = proc.take() {
        info!("Cancelling pocket-tts installation");
        let _ = child.kill();
        let _ = child.wait();
        Ok("Installation cancelled".to_string())
    } else {
        Ok("No installation in progress".to_string())
    }
}

#[tauri::command]
pub async fn pocket_tts_start(state: State<'_, AppState>) -> Result<String, String> {
    let (port, voice, temp, eos_threshold, python) = {
        let config = state.config.lock().unwrap();
        (
            config.pocket_tts.port,
            config.pocket_tts.voice.clone(),
            config.pocket_tts.temp,
            config.pocket_tts.eos_threshold,
            config.pocket_tts.python_path.clone()
                .or_else(|| find_python())
                .ok_or_else(|| "Python 3 not found".to_string())?,
        )
    };

    // Check if already running
    if check_server_running(port).await {
        return Ok("Server already running".to_string());
    }

    let script_path = get_server_script_path();
    if !script_path.exists() {
        return Err(format!(
            "Server script not found at: {}",
            script_path.display()
        ));
    }

    info!(
        "Starting pocket-tts server on port {} with voice '{}' temp={} eos={}",
        port, voice, temp, eos_threshold
    );

    let mut cmd = Command::new(&python);
    cmd.arg(script_path.to_str().unwrap_or(""))
        .args(["--port", &port.to_string()])
        .args(["--voice", &voice])
        .args(["--temp", &temp.to_string()])
        .args(["--eos-threshold", &eos_threshold.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    configure_no_window(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start server: {}", e))?;

    // Wait for the POCKET_TTS_READY signal (up to 60s for model loading)
    let stdout = child.stdout.take();
    if let Some(stdout) = stdout {
        let reader = std::io::BufReader::new(stdout);
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            for line_result in reader.lines() {
                match line_result {
                    Ok(line) => {
                        info!("[pocket-tts] {}", line);
                        if line.contains("POCKET_TTS_READY") {
                            let _ = tx.send(true);
                            return;
                        }
                        if line.contains("ERROR") {
                            let _ = tx.send(false);
                            return;
                        }
                    }
                    Err(_) => {
                        let _ = tx.send(false);
                        return;
                    }
                }
            }
            let _ = tx.send(false);
        });

        match rx.recv_timeout(std::time::Duration::from_secs(120)) {
            Ok(true) => {
                info!("pocket-tts server started successfully");

                // Store the PID for cleanup
                let mut tts_proc = state.pocket_tts_process.lock().unwrap();
                *tts_proc = Some(child);

                Ok("Server started successfully".to_string())
            }
            Ok(false) => {
                let _ = child.kill();
                Err("Server failed to start — check that pocket-tts is installed correctly".to_string())
            }
            Err(_) => {
                warn!("Timeout waiting for pocket-tts server — it may still be loading the model");
                // Keep it running, it might just be slow
                let mut tts_proc = state.pocket_tts_process.lock().unwrap();
                *tts_proc = Some(child);
                Ok("Server starting (model still loading...)".to_string())
            }
        }
    } else {
        // No stdout — just store the process and hope for the best
        let mut tts_proc = state.pocket_tts_process.lock().unwrap();
        *tts_proc = Some(child);
        Ok("Server started (no output capture)".to_string())
    }
}

#[tauri::command]
pub async fn pocket_tts_stop(state: State<'_, AppState>) -> Result<String, String> {
    let mut tts_proc = state.pocket_tts_process.lock().unwrap();
    if let Some(mut child) = tts_proc.take() {
        info!("Stopping pocket-tts server");
        let _ = child.kill();
        let _ = child.wait();
        Ok("Server stopped".to_string())
    } else {
        Ok("Server was not running".to_string())
    }
}

#[tauri::command]
pub async fn pocket_tts_voices(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let port = {
        let config = state.config.lock().unwrap();
        config.pocket_tts.port
    };

    let url = format!("http://127.0.0.1:{}/voices", port);
    match reqwest::get(&url).await {
        Ok(resp) => {
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| format!("Failed to parse voices: {}", e))?;
            Ok(body)
        }
        Err(_) => {
            // Server not running — return built-in voice list
            Ok(serde_json::json!({
                "voices": [
                    {"name": "alba", "type": "builtin", "loaded": false},
                    {"name": "marius", "type": "builtin", "loaded": false},
                    {"name": "javert", "type": "builtin", "loaded": false},
                    {"name": "jean", "type": "builtin", "loaded": false},
                    {"name": "fantine", "type": "builtin", "loaded": false},
                    {"name": "cosette", "type": "builtin", "loaded": false},
                    {"name": "eponine", "type": "builtin", "loaded": false},
                    {"name": "azelma", "type": "builtin", "loaded": false},
                ]
            }))
        }
    }
}

#[tauri::command]
pub async fn pocket_tts_test(
    _text: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let port = {
        let config = state.config.lock().unwrap();
        config.pocket_tts.port
    };

    // Just verify the server can handle a request — actual audio playback
    // happens in the frontend via fetch to the TTS server
    let url = format!("http://127.0.0.1:{}/status", port);
    match reqwest::get(&url).await {
        Ok(resp) if resp.status().is_success() => Ok(format!(
            "http://127.0.0.1:{}/tts",
            port
        )),
        _ => Err("Pocket TTS server is not running. Start it first.".to_string()),
    }
}
