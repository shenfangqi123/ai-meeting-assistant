use crate::app_config::AsrConfig;
use std::io::{BufRead, BufReader, Read};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const DEFAULT_START_TIMEOUT_SECS: u64 = 30;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum ServerDevice {
    Gpu,
    Cpu,
}

enum DevicePreference {
    Auto,
    Gpu,
    Cpu,
}

struct ServerState {
    child: Option<Child>,
    url: Option<String>,
    device: Option<ServerDevice>,
    starting: bool,
}

pub struct WhisperServerManager {
    state: Mutex<ServerState>,
}

impl WhisperServerManager {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(ServerState {
                child: None,
                url: None,
                device: None,
                starting: false,
            }),
        }
    }

    pub fn ensure_started(&self, app: &AppHandle, config: &AsrConfig) -> Result<String, String> {
        let mut guard = self
            .state
            .lock()
            .map_err(|_| "whisper-server state poisoned".to_string())?;

        if let Some(child) = guard.child.as_mut() {
            if let Ok(Some(_)) = child.try_wait() {
                guard.child = None;
                guard.url = None;
                guard.device = None;
            }
        }

        if let Some(url) = guard.url.clone() {
            return Ok(url);
        }

        if guard.starting {
            drop(guard);
            return wait_for_ready(self, Duration::from_secs(DEFAULT_START_TIMEOUT_SECS));
        }

        guard.starting = true;
        drop(guard);

        let result = start_server(app, config);

        let mut guard = self
            .state
            .lock()
            .map_err(|_| "whisper-server state poisoned".to_string())?;
        guard.starting = false;

        match result {
            Ok(handle) => {
                let device_label = match handle.device {
                    ServerDevice::Gpu => "GPU",
                    ServerDevice::Cpu => "CPU",
                };
                eprintln!("whisper-server started ({device_label}) at {}", handle.url);
                guard.url = Some(handle.url.clone());
                guard.child = Some(handle.child);
                guard.device = Some(handle.device);
                Ok(handle.url)
            }
            Err(err) => Err(err),
        }
    }

    pub fn stop(&self) {
        if let Ok(mut guard) = self.state.lock() {
            if let Some(mut child) = guard.child.take() {
                let _ = child.kill();
            }
            guard.url = None;
            guard.device = None;
            guard.starting = false;
        }
    }
}

impl Drop for WhisperServerManager {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.state.lock() {
            if let Some(mut child) = guard.child.take() {
                let _ = child.kill();
            }
        }
    }
}

struct ServerHandle {
    child: Child,
    url: String,
    device: ServerDevice,
}

fn parse_device_preference(config: &AsrConfig) -> DevicePreference {
    let raw = config
        .whisper_server_device
        .clone()
        .unwrap_or_else(|| "auto".to_string())
        .to_lowercase();
    match raw.as_str() {
        "gpu" => DevicePreference::Gpu,
        "cpu" => DevicePreference::Cpu,
        _ => DevicePreference::Auto,
    }
}

fn wait_for_ready(manager: &WhisperServerManager, timeout: Duration) -> Result<String, String> {
    let start = Instant::now();
    loop {
        {
            let guard = manager
                .state
                .lock()
                .map_err(|_| "whisper-server state poisoned".to_string())?;
            if let Some(url) = guard.url.clone() {
                return Ok(url);
            }
            if !guard.starting {
                break;
            }
        }

        if start.elapsed() > timeout {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    Err("whisper-server start timed out".to_string())
}

fn start_server(app: &AppHandle, config: &AsrConfig) -> Result<ServerHandle, String> {
    let model = resolve_model_path(app, config)
        .ok_or_else(|| "whisper-server model path not found".to_string())?;

    match parse_device_preference(config) {
        DevicePreference::Gpu => {
            let exe = resolve_server_exe(app, ServerDevice::Gpu, config)
                .ok_or_else(|| "whisper-server gpu executable not found".to_string())?;
            return spawn_server(ServerDevice::Gpu, &exe, &model);
        }
        DevicePreference::Cpu => {
            let exe = resolve_server_exe(app, ServerDevice::Cpu, config)
                .ok_or_else(|| "whisper-server cpu executable not found".to_string())?;
            return spawn_server(ServerDevice::Cpu, &exe, &model);
        }
        DevicePreference::Auto => {}
    }

    if let Some(exe) = resolve_server_exe(app, ServerDevice::Gpu, config) {
        match spawn_server(ServerDevice::Gpu, &exe, &model) {
            Ok(handle) => return Ok(handle),
            Err(err) => {
                eprintln!("whisper-server GPU failed: {err}");
            }
        }
    }

    let exe = resolve_server_exe(app, ServerDevice::Cpu, config)
        .ok_or_else(|| "whisper-server cpu executable not found".to_string())?;
    spawn_server(ServerDevice::Cpu, &exe, &model)
}

fn spawn_server(device: ServerDevice, exe: &Path, model: &Path) -> Result<ServerHandle, String> {
    if !exe.exists() {
        return Err(format!("whisper-server not found: {}", exe.display()));
    }
    if !model.exists() {
        return Err(format!("whisper model not found: {}", model.display()));
    }

    let port = pick_port()?;
    let url = format!("http://127.0.0.1:{port}/inference");
    let physical_cores = detect_physical_cores();
    let threads = recommend_threads(device, physical_cores);
    let mode = match device {
        ServerDevice::Gpu => "GPU",
        ServerDevice::Cpu => "CPU",
    };
    eprintln!(
    "whisper-server threads auto-config: mode={mode}, physical_cores={physical_cores}, -t={threads}"
  );

    let mut cmd = Command::new(exe);
    cmd.arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--inference-path")
        .arg("/inference")
        .arg("-m")
        .arg(model)
        .arg("-t")
        .arg(threads.to_string())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if device == ServerDevice::Cpu {
        cmd.arg("--no-gpu");
    }

    if let Some(dir) = exe.parent() {
        cmd.current_dir(dir);
    }

    let mut child = cmd
        .spawn()
        .map_err(|err| format!("failed to spawn whisper-server: {err}"))?;

    if let Some(stdout) = child.stdout.take() {
        spawn_reader(stdout, "whisper-server");
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_reader(stderr, "whisper-server");
    }

    wait_for_port(
        port,
        &mut child,
        Duration::from_secs(DEFAULT_START_TIMEOUT_SECS),
    )?;

    Ok(ServerHandle { child, url, device })
}

fn detect_physical_cores() -> usize {
    let physical = num_cpus::get_physical();
    if physical > 0 {
        return physical;
    }
    num_cpus::get().max(1)
}

fn recommend_threads(device: ServerDevice, physical_cores: usize) -> usize {
    match device {
        ServerDevice::Gpu => match physical_cores {
            0..=2 => 2,
            3..=4 => 3,
            5..=6 => 4,
            7..=8 => 4,
            9..=10 => 5,
            11..=12 => 8,
            13..=14 => 8,
            15..=16 => 10,
            17..=20 => 10,
            _ => 12,
        },
        ServerDevice::Cpu => match physical_cores {
            0..=2 => 2,
            3..=4 => 3,
            5..=6 => 5,
            7..=8 => 7,
            9..=12 => 10,
            13..=16 => 14,
            _ => 20,
        },
    }
}

fn spawn_reader<R: Read + Send + 'static>(reader: R, label: &'static str) {
    thread::spawn(move || {
        let mut buf = BufReader::new(reader);
        let mut line = String::new();
        loop {
            line.clear();
            let bytes = buf.read_line(&mut line).unwrap_or(0);
            if bytes == 0 {
                break;
            }
            let text = line.trim();
            if !text.is_empty() {
                eprintln!("{label}: {text}");
            }
        }
    });
}

fn wait_for_port(port: u16, child: &mut Child, timeout: Duration) -> Result<(), String> {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let start = Instant::now();
    loop {
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("whisper-server exited: {status}"));
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            return Err("whisper-server start timeout".to_string());
        }
        thread::sleep(Duration::from_millis(200));
    }
}

fn pick_port() -> Result<u16, String> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|err| err.to_string())?;
    let port = listener.local_addr().map_err(|err| err.to_string())?.port();
    Ok(port)
}

fn resolve_server_exe(
    app: &AppHandle,
    device: ServerDevice,
    config: &AsrConfig,
) -> Option<PathBuf> {
    if let Some(path) = resolve_server_path_override(app, device, config) {
        return Some(path);
    }

    let mut candidates = Vec::new();

    if let Ok(resource_dir) = app.path().resource_dir() {
        match device {
            ServerDevice::Gpu => {
                let exe = resource_dir
                    .join("whisper")
                    .join("gpu")
                    .join("120a")
                    .join("whisper-server.exe");
                let cuda = resource_dir
                    .join("whisper")
                    .join("gpu")
                    .join("120a")
                    .join("ggml-cuda.dll");
                if exe.exists() && cuda.exists() {
                    candidates.push(exe);
                }
            }
            ServerDevice::Cpu => {
                candidates.push(
                    resource_dir
                        .join("whisper")
                        .join("cpu")
                        .join("whisper-server.exe"),
                );
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        match device {
            ServerDevice::Gpu => {
                candidates.push(
                    cwd.join("install")
                        .join("gpu-120a")
                        .join("whisper-server.exe"),
                );
                candidates.push(
                    cwd.join("whisper-bin-x64")
                        .join("Release")
                        .join("whisper-server.exe"),
                );
            }
            ServerDevice::Cpu => {
                candidates.push(cwd.join("install").join("cpu").join("whisper-server.exe"));
                candidates.push(
                    cwd.join("whisper-bin-x64")
                        .join("Release")
                        .join("whisper-server.exe"),
                );
            }
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            match device {
                ServerDevice::Gpu => candidates.push(dir.join("whisper-server.exe")),
                ServerDevice::Cpu => candidates.push(dir.join("whisper-server.exe")),
            }
        }
    }

    candidates.into_iter().find(|path| path.exists())
}

fn resolve_server_path_override(
    app: &AppHandle,
    device: ServerDevice,
    config: &AsrConfig,
) -> Option<PathBuf> {
    let raw = match device {
        ServerDevice::Gpu => config
            .whisper_server_gpu_path
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                config
                    .whisper_server_path
                    .clone()
                    .filter(|value| !value.trim().is_empty())
            }),
        ServerDevice::Cpu => config
            .whisper_server_cpu_path
            .clone()
            .filter(|value| !value.trim().is_empty())
            .or_else(|| {
                config
                    .whisper_server_path
                    .clone()
                    .filter(|value| !value.trim().is_empty())
            }),
    }?;

    resolve_path_with_context(app, &raw)
}

fn resolve_path_with_context(app: &AppHandle, raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let candidate = PathBuf::from(raw);
    if candidate.is_absolute() {
        return candidate.exists().then_some(candidate);
    }

    let mut candidates = Vec::new();
    if let Ok(resource_dir) = app.path().resource_dir() {
        candidates.push(resource_dir.join(&candidate));
        if let Some(parent) = resource_dir.parent() {
            candidates.push(parent.join(&candidate));
        }
    }

    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join(&candidate));
        if let Some(parent) = cwd.parent() {
            candidates.push(parent.join(&candidate));
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join(&candidate));
            if let Some(parent) = dir.parent() {
                candidates.push(parent.join(&candidate));
            }
        }
    }

    candidates.into_iter().find(|path| path.exists())
}

fn resolve_model_path(app: &AppHandle, config: &AsrConfig) -> Option<PathBuf> {
    let raw = config
        .whisper_cpp_model_path
        .clone()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "resources/models/ggml-base.bin".to_string());
    let raw = raw.trim().to_string();
    if raw.is_empty() {
        return None;
    }

    let mut raws = Vec::new();
    raws.push(raw.clone());
    if raw != "models/ggml-small-q5_1.bin" {
        raws.push("models/ggml-small-q5_1.bin".to_string());
    }
    if raw != "resources/models/ggml-base.bin" {
        raws.push("resources/models/ggml-base.bin".to_string());
    }

    for raw in raws {
        let candidate = PathBuf::from(raw);
        if candidate.is_absolute() {
            if candidate.exists() {
                return Some(candidate);
            }
            continue;
        }

        let mut candidates = Vec::new();
        if let Ok(resource_dir) = app.path().resource_dir() {
            candidates.push(resource_dir.join(&candidate));
            if let Some(parent) = resource_dir.parent() {
                candidates.push(parent.join(&candidate));
            }
        }

        if let Ok(cwd) = std::env::current_dir() {
            candidates.push(cwd.join(&candidate));
            candidates.push(cwd.join("src-tauri").join(&candidate));
            if let Some(parent) = cwd.parent() {
                candidates.push(parent.join(&candidate));
            }
        }

        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                candidates.push(dir.join(&candidate));
                if let Some(parent) = dir.parent() {
                    candidates.push(parent.join(&candidate));
                }
            }
        }

        if let Some(found) = candidates.into_iter().find(|path| path.exists()) {
            return Some(found);
        }
    }

    None
}
