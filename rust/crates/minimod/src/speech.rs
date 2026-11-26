//! Direct-ALSA voice dictation for the native Save Project name field.

use std::ffi::{CStr, CString, c_char};
#[cfg(any(feature = "dictation", feature = "dictation-rknn"))]
use std::mem::size_of;
#[cfg(any(feature = "dictation", feature = "dictation-rknn"))]
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, AtomicI16, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use mpc_sample_sdk::{ButtonEvent, event::button};

#[cfg(feature = "dictation")]
const MOONSHINE_DIR_ENV: &str = "MINIMOD_MOONSHINE_DIR";
#[cfg(feature = "dictation")]
const MOONSHINE_WORKER_ENV: &str = "MINIMOD_MOONSHINE_WORKER";
#[cfg(feature = "dictation")]
const MOONSHINE_LIB_DIR_ENV: &str = "MINIMOD_MOONSHINE_LIB_DIR";
#[cfg(feature = "dictation-rknn")]
const RKNN_DIR_ENV: &str = "MINIMOD_RKNN_DIR";
#[cfg(feature = "dictation-rknn")]
const RKNN_WORKER_ENV: &str = "MINIMOD_RKNN_WORKER";
#[cfg(feature = "dictation-rknn")]
const RKNN_LIB_DIR_ENV: &str = "MINIMOD_RKNN_LIB_DIR";
const RATE: u32 = 16_000;
const DEFAULT_SOURCE_RATE: usize = 44_100;
const MAX_SOURCE_FRAMES: usize = 48_000 * 30;
const SAVE_CANCEL: u32 = 0x0c;
const SAVE_DO_IT: u32 = 0x0e;

static PCM_FRAMES: AtomicUsize = AtomicUsize::new(0);
static PCM_RATE: AtomicUsize = AtomicUsize::new(DEFAULT_SOURCE_RATE);
static PCM_WRITERS: AtomicUsize = AtomicUsize::new(0);
static PCM_AUDIO: [AtomicI16; MAX_SOURCE_FRAMES] = [const { AtomicI16::new(0) }; MAX_SOURCE_FRAMES];
static CAPTURE_ENABLED: AtomicBool = AtomicBool::new(false);
#[cfg(feature = "dictation")]
static MOONSHINE_WORKER: OnceLock<Mutex<Option<MoonshineWorker>>> = OnceLock::new();
#[cfg(feature = "dictation-rknn")]
static RKNN_WORKER: OnceLock<Mutex<Option<RknnWorker>>> = OnceLock::new();
static SAVE_SCREEN: AtomicBool = AtomicBool::new(false);
static EDITOR: AtomicUsize = AtomicUsize::new(0);
static SESSION: AtomicBool = AtomicBool::new(false);
static GENERATION: AtomicUsize = AtomicUsize::new(0);
// Serialize session transitions and copying the finished recording, so an old
// worker cannot reset or read a new session's shared capture buffer.
static SESSION_LOCK: Mutex<()> = Mutex::new(());
static ERROR: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static SAMPLE_RECORD_KEY_OWNED: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicPtr<c_char> = AtomicPtr::new(std::ptr::null_mut());
const PROJECT_SAVE_SCREEN_VTABLE: usize = 0x6e1faa0;
const PROJECT_SAVE_EDITOR_OFFSET: usize = 0xac0;
const TEXT_EDITOR_VTABLE: usize = 0x6e18e20;

#[cfg(feature = "dictation")]
struct MoonshineWorker {
    child: std::process::Child,
    input: std::process::ChildStdin,
    output: std::io::BufReader<std::process::ChildStdout>,
}

#[cfg(feature = "dictation")]
impl Drop for MoonshineWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Observe the text and destination pointers exposed by the SDK's native string hook.
pub(crate) fn observe_assignment(_dest: usize, text: &CStr) {
    match text.to_bytes() {
        b"Create name" => {
            // Only forget the editor. `SAVE_SCREEN` is armed by the screen's own
            // paint: this label is also assigned while the dialog is merely being
            // built at startup, and arming there swallowed SAMPLE RECORD for the
            // whole session.
            EDITOR.store(0, Ordering::Release);
        }
        _ => {}
    }
}

/// Whether dictation owns this SAMPLE RECORD press.
///
/// `save_screen` must describe the dialog *now*, not once, because the app only
/// reports the dialog closing through its own buttons; a latched `SAVE_SCREEN`
/// swallowed the plain key and left the app's recording dead for the session.
fn intercepts(event: &ButtonEvent, save_screen: bool, session: bool) -> bool {
    event.pressed && event.code == button::SAMPLE_RECORD && (save_screen || session)
}

/// Arm dictation when the save screen is painted.
///
/// The app paints a screen when it is shown and does not paint the save screen at
/// startup, so this is the reliable "the dialog is open" signal; the repaint hook
/// misses it and the `Create name` label is assigned while the dialog is merely
/// built at boot. Disarming stays with the dialog's own buttons, because the
/// Sample page also paints behind the dialog.
pub(crate) fn observe_paint(component: usize) {
    if is_project_save_screen(component) {
        SAVE_SCREEN.store(true, Ordering::Release);
    }
}

/// Handle dictation controls while the native Save Project dialog is open.
pub(crate) fn on_button(event: &ButtonEvent) -> bool {
    if !event.pressed {
        return event.code == button::SAMPLE_RECORD
            && SAMPLE_RECORD_KEY_OWNED.swap(false, Ordering::AcqRel);
    }

    if intercepts(
        event,
        SAVE_SCREEN.load(Ordering::Acquire),
        SESSION.load(Ordering::Acquire),
    ) {
        SAMPLE_RECORD_KEY_OWNED.store(true, Ordering::Release);
        if SESSION.load(Ordering::Acquire) {
            STOP_REQUESTED.store(true, Ordering::Release);
            crate::status::set_status("Voice: transcribing...");
            crate::ui::request_repaint();
        } else {
            start();
        }
        return true;
    }

    if !SAVE_SCREEN.load(Ordering::Acquire) {
        return false;
    }

    if event.code == SAVE_CANCEL || event.code == SAVE_DO_IT {
        close_screen();
    }
    false
}

/// Apply a completed transcript during the native text field's next repaint.
pub(crate) fn on_repaint() {
    if !SAVE_SCREEN.load(Ordering::Acquire) {
        return;
    }
    let editor = EDITOR.load(Ordering::Acquire);
    if editor == 0 {
        return;
    }
    let ptr = PENDING.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if ptr.is_null() {
        return;
    }
    // SAFETY: PENDING owns this CString until it is removed here.
    let text = unsafe { CString::from_raw(ptr) };
    // SAFETY: observe_repaint validates this as the current Save Project
    // TextEditor, and this callback runs on its owning UI thread.
    let assigned = unsafe { mpc_sample_sdk::replace_native_text_editor(editor, &text) };
    if assigned {
        crate::status::set_status("Voice: name inserted");
    } else {
        let replacement = text.into_raw();
        let _ = PENDING.compare_exchange(
            std::ptr::null_mut(),
            replacement,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    crate::ui::request_repaint();
}

pub(crate) fn observe_repaint(component: usize) {
    if !is_project_save_screen(component) {
        return;
    }
    // The screen paints itself when it is shown and never at startup, so this is
    // the only place that arms dictation. The `Create name` label is not usable
    // here: it is assigned while the dialog is built at startup, which armed the
    // shortcut for the whole session and swallowed the app's SAMPLE RECORD.
    SAVE_SCREEN.store(true, Ordering::Release);
    let base = mpc_sample_sdk::image::base();
    let Some(editor) = component
        .checked_add(PROJECT_SAVE_EDITOR_OFFSET)
        .and_then(mpc_sample_sdk::image::read_pointer)
    else {
        return;
    };
    if mpc_sample_sdk::image::read_pointer(editor) == Some(base + TEXT_EDITOR_VTABLE) {
        EDITOR.store(editor, Ordering::Release);
    }
}

pub(crate) fn is_project_save_screen(component: usize) -> bool {
    component != 0
        && mpc_sample_sdk::image::read_pointer(component)
            == Some(mpc_sample_sdk::image::base() + PROJECT_SAVE_SCREEN_VTABLE)
}

pub(crate) fn overlay_state() -> Option<(String, bool)> {
    if !SESSION.load(Ordering::Acquire) {
        return ERROR
            .load(Ordering::Acquire)
            .then(|| (crate::status::text(), false));
    }
    let recording =
        CAPTURE_ENABLED.load(Ordering::Acquire) && !STOP_REQUESTED.load(Ordering::Acquire);
    if recording {
        Some(("Recording...".to_owned(), true))
    } else {
        Some(("Transcribing...".to_owned(), false))
    }
}

fn start() {
    let _session = SESSION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if SESSION
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        crate::status::set_status("Voice: already recording");
        return;
    }
    ERROR.store(false, Ordering::Release);
    let generation = GENERATION.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
    STOP_REQUESTED.store(false, Ordering::Release);
    PCM_FRAMES.store(0, Ordering::Release);
    PCM_RATE.store(DEFAULT_SOURCE_RATE, Ordering::Release);
    CAPTURE_ENABLED.store(true, Ordering::Release);
    crate::status::set_status("Voice: recording; SAMPLE RECORD stops");
    crate::ui::request_repaint();
    if std::thread::Builder::new()
        .name("minimod-voice-input".to_owned())
        .spawn(move || run(generation))
        .is_err()
    {
        CAPTURE_ENABLED.store(false, Ordering::Release);
        SESSION.store(false, Ordering::Release);
        crate::status::set_status("Voice: could not start");
    }
}

fn run(generation: usize) {
    // Button callbacks run on a real-time firmware thread. New pthreads inherit
    // that scheduling policy, so explicitly leave it before inference creates
    // its own workers; otherwise they compete with the audio/UI threads.
    #[cfg(target_os = "linux")]
    {
        let param = libc::sched_param { sched_priority: 0 };
        // SAFETY: the parameter points to a valid SCHED_OTHER configuration for this thread.
        let result =
            unsafe { libc::pthread_setschedparam(libc::pthread_self(), libc::SCHED_OTHER, &param) };
        if result != 0 {
            let _session = SESSION_LOCK
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if GENERATION.load(Ordering::Acquire) == generation {
                crate::status::set_status("Voice: worker scheduling failed");
                ERROR.store(true, Ordering::Release);
                CAPTURE_ENABLED.store(false, Ordering::Release);
                SESSION.store(false, Ordering::Release);
                crate::ui::request_repaint();
            }
            return;
        }
    }
    #[cfg(feature = "dictation-rknn")]
    if let Err(error) = rknn_worker() {
        crate::status::set_status(&format!("Voice: {error}"));
    }
    #[cfg(feature = "dictation")]
    if let Err(error) = moonshine_worker() {
        crate::status::set_status(&format!("Voice: {error}"));
    }
    let result = capture(generation).and_then(|audio| transcribe(audio, generation));
    let _session = SESSION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if GENERATION.load(Ordering::Acquire) != generation || !SAVE_SCREEN.load(Ordering::Acquire) {
        // The dialog went away mid-session. Drop this session instead of returning
        // with `SESSION` still set, which would keep the dictation gesture armed
        // for the rest of the run.
        if GENERATION.load(Ordering::Acquire) == generation {
            CAPTURE_ENABLED.store(false, Ordering::Release);
            SESSION.store(false, Ordering::Release);
            crate::ui::request_repaint();
        }
        return;
    }
    match result {
        Ok(name) => match CString::new(name) {
            Ok(text) => {
                let replacement = text.into_raw();
                let old = PENDING.swap(replacement, Ordering::AcqRel);
                if !old.is_null() {
                    // SAFETY: PENDING returned the prior owned CString pointer.
                    unsafe { drop(CString::from_raw(old)) };
                }
                crate::status::set_status("Voice: name ready");
            }
            Err(_) => crate::status::set_status("Voice: invalid name"),
        },
        Err(error) => {
            crate::status::set_status(&format!("Voice: {error}"));
            ERROR.store(true, Ordering::Release);
        }
    }
    SESSION.store(false, Ordering::Release);
    crate::ui::request_repaint();
}

fn capture(generation: usize) -> Result<Vec<f32>, String> {
    while !STOP_REQUESTED.load(Ordering::Acquire)
        && PCM_FRAMES.load(Ordering::Acquire) < MAX_SOURCE_FRAMES
        && GENERATION.load(Ordering::Acquire) == generation
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let session = SESSION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if GENERATION.load(Ordering::Acquire) != generation {
        return Err("cancelled".to_owned());
    }
    CAPTURE_ENABLED.store(false, Ordering::Release);
    while PCM_WRITERS.load(Ordering::Acquire) != 0 {
        std::thread::yield_now();
    }
    crate::ui::request_repaint();
    let count = PCM_FRAMES.load(Ordering::Acquire).min(MAX_SOURCE_FRAMES);
    STOP_REQUESTED.store(false, Ordering::Release);
    let source_rate = PCM_RATE.load(Ordering::Acquire).max(1);
    if count < source_rate / 2 {
        return Err("recording was too short".to_owned());
    }
    let source: Vec<f32> = PCM_AUDIO[..count]
        .iter()
        .map(|sample| f32::from(sample.load(Ordering::Acquire)) / 32768.0)
        .collect();
    drop(session);
    let output_len = count * RATE as usize / source_rate;
    let audio: Vec<f32> = (0..output_len)
        .map(|index| {
            let position = index as f32 * source_rate as f32 / RATE as f32;
            let left = position as usize;
            let fraction = position - left as f32;
            let a = source[left.min(count - 1)];
            let b = source[(left + 1).min(count - 1)];
            a + (b - a) * fraction
        })
        .collect();
    trim_silence(audio).ok_or_else(|| "no speech detected".to_owned())
}

fn trim_silence(audio: Vec<f32>) -> Option<Vec<f32>> {
    const WINDOW: usize = 320; // 20 ms at Speech models 16 kHz input rate
    const RMS_FLOOR: f32 = 0.008;
    const PADDING: usize = RATE as usize / 5; // Keep 200 ms around the speech.

    let mut first = None;
    let mut last = 0;
    for (window, samples) in audio.chunks(WINDOW).enumerate() {
        let rms = (samples.iter().map(|sample| sample * sample).sum::<f32>()
            / samples.len() as f32)
            .sqrt();
        if rms >= RMS_FLOOR {
            first.get_or_insert(window * WINDOW);
            last = (window + 1) * WINDOW;
        }
    }
    let first = first?;
    let start = first.saturating_sub(PADDING);
    let end = last.saturating_add(PADDING).min(audio.len());
    Some(audio[start..end].to_vec())
}

/// Called by the SDK after the firmware's ALSA read succeeds. The SDK forwards
/// this callback directly from its `snd_pcm_readi` interposer.
pub(crate) fn input_interleaved(samples: &[i32], channels: usize, rate: u32) {
    if !CAPTURE_ENABLED.load(Ordering::Acquire) || channels == 0 || rate == 0 {
        return;
    }
    PCM_WRITERS.fetch_add(1, Ordering::AcqRel);
    if !CAPTURE_ENABLED.load(Ordering::Acquire) {
        PCM_WRITERS.fetch_sub(1, Ordering::Release);
        return;
    }
    PCM_RATE.store(rate as usize, Ordering::Release);
    let count = samples.len() / channels;
    let start = PCM_FRAMES.fetch_add(count, Ordering::AcqRel);
    let writable = count.min(MAX_SOURCE_FRAMES.saturating_sub(start));
    for frame in 0..writable {
        let l = samples[frame * channels] >> 16;
        let r = samples[frame * channels + usize::from(channels > 1)] >> 16;
        PCM_AUDIO[start + frame].store(((l + r) / 2) as i16, Ordering::Release);
    }
    PCM_WRITERS.fetch_sub(1, Ordering::Release);
}

#[cfg(feature = "dictation-rknn")]
struct RknnWorker {
    child: std::process::Child,
    input: std::process::ChildStdin,
    output: std::io::BufReader<std::process::ChildStdout>,
}

#[cfg(feature = "dictation")]
fn moonshine_worker() -> Result<std::sync::MutexGuard<'static, Option<MoonshineWorker>>, String> {
    let workers = MOONSHINE_WORKER.get_or_init(|| Mutex::new(None));
    let mut worker = workers.lock().map_err(|_| "Moonshine worker lock failed")?;
    if worker.is_none() {
        let model_directory = std::env::var_os(MOONSHINE_DIR_ENV)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("missing {MOONSHINE_DIR_ENV}"))?;
        let executable = std::env::var_os(MOONSHINE_WORKER_ENV)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| {
                std::path::PathBuf::from("/run/minimod-moonshine/minimod-moonshine")
            });
        let library_directory = std::env::var_os(MOONSHINE_LIB_DIR_ENV)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from("/run/minimod-moonshine/lib"));
        let library_path = std::env::var_os("LD_LIBRARY_PATH")
            .map(|path| format!("{}:{}", library_directory.display(), path.to_string_lossy()))
            .unwrap_or_else(|| library_directory.display().to_string());
        let mut child = std::process::Command::new(executable)
            .current_dir(&model_directory)
            .arg(&model_directory)
            .env("LD_LIBRARY_PATH", library_path)
            .env_remove("LD_PRELOAD")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start Moonshine worker: {error}"))?;
        let input = child
            .stdin
            .take()
            .ok_or("Moonshine worker input unavailable")?;
        let output = child
            .stdout
            .take()
            .ok_or("Moonshine worker output unavailable")?;
        let mut output = std::io::BufReader::new(output);
        let ready = match worker_read_line(&mut output, 30_000) {
            Ok(line) => line,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if ready.trim_end() != "READY" {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ready
                .strip_prefix("ERR|")
                .unwrap_or("Moonshine worker initialization failed")
                .trim()
                .to_owned());
        }
        *worker = Some(MoonshineWorker {
            child,
            input,
            output,
        });
    }
    Ok(worker)
}

#[cfg(feature = "dictation")]
fn send_moonshine_audio(worker: &mut MoonshineWorker, audio: &[f32]) -> Result<String, String> {
    use std::io::Write;

    writeln!(worker.input, "{}", audio.len()).map_err(|_| "Moonshine worker stopped")?;
    for block in audio.chunks(4096) {
        let mut bytes = Vec::with_capacity(block.len() * size_of::<f32>());
        for sample in block {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        worker
            .input
            .write_all(&bytes)
            .map_err(|_| "Moonshine worker stopped")?;
    }
    worker
        .input
        .flush()
        .map_err(|_| "Moonshine worker stopped")?;

    worker_read_line(&mut worker.output, 30_000)
}

#[cfg(feature = "dictation-rknn")]
impl Drop for RknnWorker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(feature = "dictation-rknn")]
fn rknn_worker() -> Result<std::sync::MutexGuard<'static, Option<RknnWorker>>, String> {
    let workers = RKNN_WORKER.get_or_init(|| Mutex::new(None));
    let mut worker = workers.lock().map_err(|_| "NPU worker lock failed")?;
    if worker.is_none() {
        let directory = std::env::var_os(RKNN_DIR_ENV)
            .map(std::path::PathBuf::from)
            .ok_or_else(|| format!("missing {RKNN_DIR_ENV}"))?;
        let executable = std::env::var_os(RKNN_WORKER_ENV)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| directory.join("minimod-whisper-rknn"));
        let library_directory = std::env::var_os(RKNN_LIB_DIR_ENV)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| directory.join("lib"));
        let model_directory = directory.join("model");
        let library_path = std::env::var_os("LD_LIBRARY_PATH")
            .map(|path| format!("{}:{}", library_directory.display(), path.to_string_lossy()))
            .unwrap_or_else(|| library_directory.display().to_string());
        let mut child = std::process::Command::new(executable)
            .current_dir(&directory)
            .arg(model_directory.join("whisper_encoder_tiny.en_20s.rknn"))
            .arg(model_directory.join("whisper_decoder_tiny.en_20s.rknn"))
            .arg(model_directory.join("vocab_en.txt"))
            .arg(model_directory.join("mel_80_filters.txt"))
            .env("LD_LIBRARY_PATH", library_path)
            .env_remove("LD_PRELOAD")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("could not start RKNN worker: {error}"))?;
        let input = child.stdin.take().ok_or("RKNN worker input unavailable")?;
        let output = child
            .stdout
            .take()
            .ok_or("RKNN worker output unavailable")?;
        let mut output = std::io::BufReader::new(output);
        let ready = match worker_read_line(&mut output, 30_000) {
            Ok(line) => line,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        if ready.trim_end() != "READY" {
            let _ = child.kill();
            let _ = child.wait();
            return Err(ready
                .strip_prefix("ERR|")
                .unwrap_or("RKNN worker initialization failed")
                .trim()
                .to_owned());
        }
        *worker = Some(RknnWorker {
            child,
            input,
            output,
        });
    }
    Ok(worker)
}

#[cfg(feature = "dictation-rknn")]
fn transcribe(audio: Vec<f32>, _generation: usize) -> Result<String, String> {
    let mut worker = rknn_worker()?;
    let result = send_rknn_audio(worker.as_mut().ok_or("RKNN worker unavailable")?, &audio);
    if result.is_err() {
        worker.take();
    }
    let response = result?;
    let response = response.trim_end();
    if let Some(text) = response.strip_prefix("OK|") {
        let name = clean_name(text);
        if name.is_empty() {
            Err("no speech recognized".to_owned())
        } else {
            Ok(name)
        }
    } else if let Some(error) = response.strip_prefix("ERR|") {
        Err(error.to_owned())
    } else {
        Err("invalid RKNN worker response".to_owned())
    }
}

#[cfg(feature = "dictation-rknn")]
fn send_rknn_audio(worker: &mut RknnWorker, audio: &[f32]) -> Result<String, String> {
    use std::io::Write;

    writeln!(worker.input, "{}", audio.len()).map_err(|_| "RKNN worker stopped")?;
    for block in audio.chunks(4096) {
        let mut bytes = Vec::with_capacity(block.len() * size_of::<f32>());
        for sample in block {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        worker
            .input
            .write_all(&bytes)
            .map_err(|_| "RKNN worker stopped")?;
    }
    worker.input.flush().map_err(|_| "RKNN worker stopped")?;

    worker_read_line(&mut worker.output, 30_000)
}

#[cfg(any(feature = "dictation", feature = "dictation-rknn"))]
fn worker_read_line(
    output: &mut std::io::BufReader<std::process::ChildStdout>,
    timeout_ms: i32,
) -> Result<String, String> {
    use std::io::BufRead;

    let mut descriptors = [libc::pollfd {
        fd: output.get_ref().as_raw_fd(),
        events: libc::POLLIN | libc::POLLHUP,
        revents: 0,
    }];
    // SAFETY: `descriptors` points to one initialized pollfd for the duration of the call.
    let ready = unsafe { libc::poll(descriptors.as_mut_ptr(), 1, timeout_ms) };
    if ready == 0 {
        return Err("speech worker response timed out".to_owned());
    }
    if ready < 0 {
        return Err("speech worker poll failed".to_owned());
    }
    let mut line = String::new();
    if output
        .read_line(&mut line)
        .map_err(|_| "speech worker stopped")?
        == 0
    {
        return Err("speech worker stopped".to_owned());
    }
    Ok(line)
}

#[cfg(feature = "dictation")]
fn transcribe(audio: Vec<f32>, generation: usize) -> Result<String, String> {
    if GENERATION.load(Ordering::Acquire) != generation {
        return Err("cancelled".to_owned());
    }
    let mut worker = moonshine_worker()?;
    let result = send_moonshine_audio(
        worker.as_mut().ok_or("Moonshine worker unavailable")?,
        &audio,
    );
    if result.is_err() {
        worker.take();
    }
    let response = result?;
    let response = response.trim_end();
    if let Some(text) = response.strip_prefix("OK|") {
        let name = clean_name(text);
        if name.is_empty() {
            Err("no speech recognized".to_owned())
        } else {
            Ok(name)
        }
    } else if let Some(error) = response.strip_prefix("ERR|") {
        Err(error.to_owned())
    } else {
        Err("invalid Moonshine worker response".to_owned())
    }
}

fn clean_name(text: &str) -> String {
    let mut name = String::new();
    for word in text.split_whitespace() {
        let mut chars = word.chars().filter(char::is_ascii_alphanumeric);
        if let Some(first) = chars.next() {
            name.extend(first.to_uppercase());
            name.extend(chars);
        }
    }
    name
}

fn close_screen() {
    let _session = SESSION_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    GENERATION.fetch_add(1, Ordering::AcqRel);
    SAVE_SCREEN.store(false, Ordering::Release);
    ERROR.store(false, Ordering::Release);
    EDITOR.store(0, Ordering::Release);
    STOP_REQUESTED.store(true, Ordering::Release);
    CAPTURE_ENABLED.store(false, Ordering::Release);
    SESSION.store(false, Ordering::Release);
    let old = PENDING.swap(std::ptr::null_mut(), Ordering::AcqRel);
    if !old.is_null() {
        // SAFETY: PENDING owned this CString and no native assignment received it.
        unsafe { drop(CString::from_raw(old)) };
    }
}

#[cfg(test)]
mod tests {
    use super::{button, intercepts};
    use crate::buttons::PRESSED;
    use mpc_sample_sdk::ButtonEvent;

    fn sample_record(pressed: bool) -> ButtonEvent {
        ButtonEvent {
            event: 0,
            bytes: [0; 16],
            code: button::SAMPLE_RECORD,
            value: if pressed { PRESSED } else { 0 },
            pressed,
            shift: false,
            erase: false,
            surface: 1,
        }
    }

    #[test]
    fn dictation_takes_the_plain_key_only_while_the_dialog_is_open() {
        assert!(intercepts(&sample_record(true), true, false));
    }

    #[test]
    fn a_button_press_outside_the_dialog_belongs_to_the_app() {
        // The regression: a stale latch swallowed the plain key everywhere.
        assert!(!intercepts(&sample_record(true), false, false));
    }

    #[test]
    fn an_open_session_takes_the_key_to_stop() {
        assert!(intercepts(&sample_record(true), false, true));
    }

    #[test]
    fn releases_are_never_intercepted_here() {
        assert!(!intercepts(&sample_record(false), true, true));
    }
}
