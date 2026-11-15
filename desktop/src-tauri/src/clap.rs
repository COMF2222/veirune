//! Optional, local ONNX CLAP inference. A verified component set supplies the
//! worker and weights; audio samples are deleted after each request.

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Mutex, OnceLock},
    time::{Duration, Instant},
};

static WORKER: OnceLock<Mutex<Option<Worker>>> = OnceLock::new();
static BUNDLE_DIR: OnceLock<PathBuf> = OnceLock::new();
static RESOURCES: OnceLock<std::sync::Arc<crate::components::Manager>> = OnceLock::new();

pub fn configure(resource_dir: &Path, resources: std::sync::Arc<crate::components::Manager>) {
    let _ = BUNDLE_DIR.set(resource_dir.join("resources").join("clap"));
    let _ = RESOURCES.set(resources);
}

struct Worker {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    last_used: Instant,
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn root() -> Result<PathBuf> {
    Ok(crate::config::app_paths()?.root)
}

fn runtime() -> Option<(PathBuf, PathBuf)> {
    let base = std::env::var_os("FASTCLOUD_CLAP_DIR")
        .map(PathBuf::from).or_else(|| {
            if let Some(resources) = RESOURCES.get() && resources.managed() {
                resources.active()
            } else { BUNDLE_DIR.get().cloned() }
        })?;
    let worker = base.join(crate::components::WORKER_PATH);
    let model = base.join("model");
    (worker.is_file() && model.join("onnx").join("audio_model_quantized.onnx").is_file()
        && model.join("onnx").join("text_model_quantized.onnx").is_file())
        .then_some((worker, model))
}

pub fn available() -> bool { runtime().is_some() }

/// Installer resources keep the executable bit, but a worker that was unpacked
/// from a downloaded artifact archive may lose it. Restore it before spawning so
/// a bundled Linux build reports CLAP inference instead of "permission denied".
fn ensure_worker_executable(worker: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let permissions = std::fs::metadata(worker)
            .with_context(|| format!("inspect CLAP worker {}", worker.display()))?
            .permissions();
        if permissions.mode() & 0o111 != 0o111 {
            std::fs::set_permissions(worker, std::fs::Permissions::from_mode(0o755)).with_context(
                || format!("make CLAP worker executable {}", worker.display()),
            )?;
        }
    }
    #[cfg(not(unix))]
    let _ = worker;
    Ok(())
}

pub fn release_if_idle() {
    if let Some(lock) = WORKER.get()
        && let Ok(mut worker) = lock.try_lock()
        && worker.as_ref().is_some_and(|current| current.last_used.elapsed() > Duration::from_secs(180)) {
        *worker = None;
    }
}

impl Worker {
    fn start() -> Result<Self> {
        let (executable, model) = runtime().context("bundled CLAP runtime is unavailable")?;
        ensure_worker_executable(&executable)?;
        let root = root()?;
        std::fs::create_dir_all(&root)?;
        let log = std::fs::File::create(root.join("clap-worker.log"))?;
        let mut command = Command::new(executable);
        command.arg(model)
            .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::from(log));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000 | 0x0000_4000); // No window, below-normal CPU priority
        }
        let mut child = command.spawn().context("start CLAP worker")?;
        let stdin = child.stdin.take().context("CLAP worker stdin")?;
        let stdout = child.stdout.take().context("CLAP worker stdout")?;
        let mut worker = Self { child, stdin, stdout: BufReader::new(stdout), last_used: Instant::now() };
        let mut line = String::new();
        worker.stdout.read_line(&mut line).context("wait for CLAP model")?;
        let ready: Value = serde_json::from_str(&line).context("CLAP worker did not load")?;
        ensure!(ready["ready"] == true, "CLAP worker could not load the model");
        log::info!("CLAP model loaded on {}", ready["device"]);
        Ok(worker)
    }

    fn request(&mut self, value: &Value) -> Result<Vec<Vec<f32>>> {
        serde_json::to_writer(&mut self.stdin, value)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let mut line = String::new();
        ensure!(self.stdout.read_line(&mut line)? > 0, "CLAP worker exited");
        let response: Value = serde_json::from_str(&line)?;
        if let Some(error) = response["error"].as_str() { anyhow::bail!("{error}"); }
        let vectors: Vec<Vec<f32>> = serde_json::from_value(response["vectors"].clone())?;
        ensure!(!vectors.is_empty() && vectors.iter().all(|vector|
            vector.len() >= 256 && vector.len() <= 1024 && vector.iter().all(|v| v.is_finite())),
            "CLAP returned invalid embeddings");
        self.last_used = Instant::now();
        Ok(vectors)
    }
}

fn request(value: Value) -> Result<Vec<Vec<f32>>> {
    let mut guard = WORKER.get_or_init(|| Mutex::new(None))
        .lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    for attempt in 0..2 {
        if guard.is_none() { *guard = Some(Worker::start()?); }
        if let Some(worker) = guard.as_mut() {
            match worker.request(&value) {
                Ok(vectors) => return Ok(vectors),
                Err(error) if attempt == 0 => {
                    log::warn!("CLAP worker retry after error: {error}");
                    *guard = None;
                }
                Err(error) => return Err(error),
            }
        }
    }
    anyhow::bail!("CLAP worker unavailable")
}

pub fn embed_texts(prompts: &[String]) -> Result<Vec<Vec<f32>>> {
    if prompts.is_empty() { return Ok(Vec::new()); }
    ensure!(prompts.len() <= 128, "CLAP text batch too large");
    request(json!({"kind": "text", "prompts": prompts}))
}

fn write_samples(path: &Path, samples: &[f32]) -> Result<()> {
    let file = std::fs::File::create(path)?;
    let mut writer = BufWriter::new(file);
    for sample in samples {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
}

pub fn embed_audio(track_id: u64, samples: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
    ensure!(sample_rate >= 8_000 && sample_rate <= 192_000, "invalid sample rate");
    ensure!(samples.len() >= sample_rate as usize * 5, "audio sample too short");
    let dir = root()?.join("wave-temp");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{track_id}-{}.f32", std::process::id()));
    write_samples(&path, samples)?;
    let result = request(json!({"kind": "audio", "path": path, "rate": sample_rate}));
    let _ = std::fs::remove_file(&path);
    result?.into_iter().next().context("CLAP returned no audio embedding")
}
