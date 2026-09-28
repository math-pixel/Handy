// Manages an optionally auto-started Ollama server for headless post-processing.

use log::{debug, info};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// Stores the spawned `ollama serve` child process, if we started it.
static OLLAMA_PROCESS: OnceLock<Mutex<Option<Child>>> = OnceLock::new();

fn process_store() -> &'static Mutex<Option<Child>> {
    OLLAMA_PROCESS.get_or_init(|| Mutex::new(None))
}

/// Returns true if Ollama's HTTP server is already accepting connections.
fn is_server_running() -> bool {
    // reqwest is async; use a plain TCP connect instead.
    std::net::TcpStream::connect_timeout(
        &"127.0.0.1:11434".parse().unwrap(),
        Duration::from_millis(500),
    )
    .is_ok()
}

/// Ensure `ollama serve` is running. Spawns it if not already up.
/// Waits up to 10 s for the server to become reachable.
pub fn ensure_server_running() -> Result<(), String> {
    if is_server_running() {
        debug!("ollama_headless: server already running");
        return Ok(());
    }

    info!("ollama_headless: server not detected — spawning `ollama serve`");

    let child = Command::new("ollama")
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn `ollama serve`: {e}"))?;

    let mut guard = process_store()
        .lock()
        .map_err(|_| "process_store mutex poisoned".to_string())?;
    *guard = Some(child);
    drop(guard);

    // Poll until the server is ready (up to 10 s).
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if is_server_running() {
            info!("ollama_headless: server ready");
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    Err("Timed out waiting for `ollama serve` to become ready (10 s)".to_string())
}

/// List models installed locally via `ollama list`.
/// Parses the table output; works even before the server has started.
pub fn list_models() -> Result<Vec<String>, String> {
    let output = Command::new("ollama")
        .arg("list")
        .output()
        .map_err(|e| format!("Failed to run `ollama list`: {e}. Is Ollama installed?"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`ollama list` failed: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    // Skip header line; NAME is the first column.
    let models: Vec<String> = stdout
        .lines()
        .skip(1)
        .filter_map(|line| {
            let name = line.split_whitespace().next()?;
            if name.is_empty() {
                None
            } else {
                Some(name.to_string())
            }
        })
        .collect();

    Ok(models)
}
