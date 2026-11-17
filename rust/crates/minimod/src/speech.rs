//! Direct-ALSA voice dictation for the native Save Project name field.

use std::ffi::{CStr, CString, c_char};
use std::sync::atomic::{AtomicBool, AtomicI16, AtomicPtr, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use mpc_sample_sdk::{ButtonEvent, event::button};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const MODEL_ENV: &str = "MINIMOD_WHISPER_MODEL";
const MODEL_NAME: &str = "ggml-tiny.en-q5_1.bin";
const RATE: u32 = 16_000;
const DEFAULT_SOURCE_RATE: usize = 44_100;
const MAX_SOURCE_FRAMES: usize = 48_000 * 30;
const SAVE_CANCEL: u32 = 0x0c;
const SAVE_DO_IT: u32 = 0x0e;

static PCM_FRAMES: AtomicUsize = AtomicUsize::new(0);
static PCM_RATE: AtomicUsize = AtomicUsize::new(DEFAULT_SOURCE_RATE);
static PCM_AUDIO: [AtomicI16; MAX_SOURCE_FRAMES] = [const { AtomicI16::new(0) }; MAX_SOURCE_FRAMES];
static CAPTURE_ENABLED: AtomicBool = AtomicBool::new(false);
static WHISPER: OnceLock<Mutex<Option<WhisperContext>>> = OnceLock::new();
static SAVE_SCREEN: AtomicBool = AtomicBool::new(false);
static EDITOR: AtomicUsize = AtomicUsize::new(0);
static SESSION: AtomicBool = AtomicBool::new(false);
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);
static SAMPLE_RECORD_KEY_OWNED: AtomicBool = AtomicBool::new(false);
static PENDING: AtomicPtr<c_char> = AtomicPtr::new(std::ptr::null_mut());
const PROJECT_SAVE_SCREEN_VTABLE: usize = 0x6e1faa0;
const PROJECT_SAVE_EDITOR_OFFSET: usize = 0xac0;
const TEXT_EDITOR_VTABLE: usize = 0x6e18e20;

/// Observe the text and destination pointers exposed by the SDK's native string hook.
pub(crate) fn observe_assignment(_dest: usize, text: &CStr) {
    match text.to_bytes() {
        b"Create name" => {
            EDITOR.store(0, Ordering::Release);
            SAVE_SCREEN.store(true, Ordering::Release);
        }
        _ => {}
    }
}

/// Handle dictation controls while the native Save Project dialog is open.
pub(crate) fn on_button(event: &ButtonEvent) -> bool {
    if !event.pressed {
        return event.code == button::SAMPLE_RECORD
            && SAMPLE_RECORD_KEY_OWNED.swap(false, Ordering::AcqRel);
    }

    if event.code == button::SAMPLE_RECORD
        && (SAVE_SCREEN.load(Ordering::Acquire) || SESSION.load(Ordering::Acquire))
    {
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
    // The dialog may already exist before the mod is loaded, and its static
    // "Create name" label is not necessarily assigned again. Detect the live
    // native screen itself so the shortcuts work whenever the field is open.
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

pub(crate) fn overlay_state() -> Option<(&'static str, bool)> {
    if !SESSION.load(Ordering::Acquire) {
        return None;
    }
    let recording = CAPTURE_ENABLED.load(Ordering::Acquire)
        && !STOP_REQUESTED.load(Ordering::Acquire);
    if recording {
        Some(("Recording...", true))
    } else {
        Some(("Transcribing...", false))
    }
}

fn start() {
    if SESSION
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        crate::status::set_status("Voice: already recording");
        return;
    }
    STOP_REQUESTED.store(false, Ordering::Release);
    PCM_FRAMES.store(0, Ordering::Release);
    PCM_RATE.store(DEFAULT_SOURCE_RATE, Ordering::Release);
    CAPTURE_ENABLED.store(true, Ordering::Release);
    crate::status::set_status("Voice: recording; SAMPLE RECORD stops");
    crate::ui::request_repaint();
    if std::thread::Builder::new()
        .name("minimod-voice-input".to_owned())
        .spawn(run)
        .is_err()
    {
        CAPTURE_ENABLED.store(false, Ordering::Release);
        SESSION.store(false, Ordering::Release);
        crate::status::set_status("Voice: could not start");
    }
}

fn run() {
    // Inference is background work; leave the audio and UI threads responsive.
    #[cfg(target_os = "linux")]
    unsafe { libc::setpriority(libc::PRIO_PROCESS, 0, 10) };
    let result = capture().and_then(transcribe);
    if !SAVE_SCREEN.load(Ordering::Acquire) {
        SESSION.store(false, Ordering::Release);
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
        Err(error) => crate::status::set_status(&format!("Voice: {error}")),
    }
    SESSION.store(false, Ordering::Release);
    crate::ui::request_repaint();
}

fn capture() -> Result<Vec<f32>, String> {
    while !STOP_REQUESTED.load(Ordering::Acquire)
        && PCM_FRAMES.load(Ordering::Acquire) < MAX_SOURCE_FRAMES
    {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    CAPTURE_ENABLED.store(false, Ordering::Release);
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
    const WINDOW: usize = 320; // 20 ms at Whisper's 16 kHz input rate
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
    PCM_RATE.store(rate as usize, Ordering::Release);
    let count = samples.len() / channels;
    let start = PCM_FRAMES.fetch_add(count, Ordering::AcqRel);
    let writable = count.min(MAX_SOURCE_FRAMES.saturating_sub(start));
    for frame in 0..writable {
        let l = samples[frame * channels] >> 16;
        let r = samples[frame * channels + usize::from(channels > 1)] >> 16;
        PCM_AUDIO[start + frame].store(((l + r) / 2) as i16, Ordering::Release);
    }
}

fn transcribe(audio: Vec<f32>) -> Result<String, String> {
    let model = std::env::var_os(MODEL_ENV)
        .map(std::path::PathBuf::from)
        .or_else(|| {
            ["/run/minimod-whisper.bin", "/media/MPC SAMPLE/MINIMOD/ggml-tiny.en-q5_1.bin"]
                .into_iter()
                .map(std::path::PathBuf::from)
                .find(|path| path.is_file())
        })
        .ok_or_else(|| format!("missing {MODEL_NAME}"))?;
    let lock = WHISPER.get_or_init(|| Mutex::new(None));
    let mut context = lock.lock().map_err(|_| "model lock failed")?;
    if context.is_none() {
        *context = Some(
            WhisperContext::new_with_params(&model, WhisperContextParameters::default())
                .map_err(|_| "model load failed")?,
        );
    }
    let context = context.as_ref().ok_or("model unavailable")?;
    let mut state = context.create_state().map_err(|_| "model state failed")?;
    let mut params = FullParams::new(SamplingStrategy::BeamSearch {
        beam_size: 5,
        patience: -1.0,
    });
    params.set_language(Some("en"));
    params.set_n_threads(1);
    params.set_no_context(true);
    params.set_suppress_blank(true);
    params.set_print_special(false);
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_timestamps(false);
    state.full(params, &audio).map_err(|_| "transcription failed")?;
    let text = state.as_iter().map(|segment| segment.to_string()).collect::<String>();
    let name = clean_name(&text);
    if name.is_empty() {
        return Err("no speech recognized".to_owned());
    }
    Ok(name)
}

fn clean_name(text: &str) -> String {
    let mut name = String::new();
    for word in text.split_whitespace() {
        let mut chars = word.chars().filter(char::is_ascii_alphanumeric);
        if let Some(first) = chars.next() {
            name.extend(first.to_uppercase());
            name.extend(chars);
        }
        if name.len() >= 32 {
            name.truncate(32);
            break;
        }
    }
    name
}

fn close_screen() {
    SAVE_SCREEN.store(false, Ordering::Release);
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
