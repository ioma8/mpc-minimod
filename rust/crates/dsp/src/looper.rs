//! The live looper engine.
//!
//! Sixteen pads, each holding one mono loop recorded from the firmware's audio
//! input and mixed into its audio output.  The engine is a process singleton
//! because it is driven by two real audio callbacks that exist exactly once in
//! the process: the capture hook and the playback hook.
//!
//! # Threading
//!
//! * the capture hook calls [`Engine::input_s32`]/[`Engine::input_s32_interleaved`]
//!   and is the only writer of the input ring;
//! * the playback hook calls [`Engine::render_s32`], which is the clock: every
//!   played frame advances one transport counter, so a loop length is always an
//!   exact number of played frames;
//! * pad and button events call [`Engine::pad`]/[`Engine::button`], which change
//!   state but never touch audio buffers while a take is in progress. The caller
//!   must serialize these control events, as the SDK does for UI callbacks;
//! * the paint hook calls [`Engine::snapshot`], which only reads.
//!
//! No path allocates or locks.  The loop pool is mapped once when the view
//! opens and every cross-thread field is published with a release store and read
//! with an acquire load.
//!
//! # Latency
//!
//! A take uses the latency estimate supplied by the SDK. It drops the initial
//! estimated latency and captures a tail after the closing press
//! ([`State::Drain`]); overdubs use the same estimate to shift input into the
//! loop. Buffer geometry alone does not prove sample-accurate device latency.

use core::fmt::Write as _;
use core::mem::size_of;
use core::sync::atomic::{
    AtomicBool, AtomicI32, AtomicI64, AtomicPtr, AtomicU8, AtomicU32, AtomicU64, Ordering,
};

use crate::sample::{s32_pair_to_f32, s32_to_f32};
use crate::time::now_ms;

/// Number of pad loops.
pub const PADS: usize = 16;
/// Sample rate of the audio device the engine runs behind.
pub const RATE: u32 = 44_100;
/// Longest loop, in frames (eight seconds).
pub const MAX_FRAMES: usize = (RATE as usize) * 8;
/// Size of the mono input ring, in frames.
const RING: u32 = 32_768;
/// Frames per DSP block; bounds the stack scratch of both audio paths.
const BLOCK: usize = 512;
/// Playback gain applied to every loop before the master volume.
const GAIN: f32 = 0.85;
/// Length of one metronome click, in frames.
const CLICK_FRAMES: u64 = 420;
/// Largest record latency the engine accepts, in frames (~370 ms).
pub const LAT_MAX: u32 = 16_384;
/// Capacity of the status line, including the terminator.
pub const MESSAGE_LEN: usize = 40;
/// Headroom a snapped length may gain before it counts as truncated material.
const QUANTIZE_SLACK: u32 = 8;

/// State of one pad.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// No loop on this pad.
    Idle = 0,
    /// Recording a new take.
    Rec = 1,
    /// Looping.
    Play = 2,
    /// Looping and adding the input on top.
    Dub = 3,
    /// A loop is kept but not playing.
    Stop = 4,
    /// A take was closed and is still capturing its latency tail.
    Drain = 5,
}

impl State {
    /// Rebuild a state from its published integer.
    #[must_use]
    pub const fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::Rec,
            2 => Self::Play,
            3 => Self::Dub,
            4 => Self::Stop,
            5 => Self::Drain,
            _ => Self::Idle,
        }
    }

    /// The published integer for this state.
    #[must_use]
    pub const fn raw(self) -> i32 {
        self as i32
    }

    /// Whether a take is in progress (recording, overdubbing or draining).
    #[must_use]
    pub const fn is_take(self) -> bool {
        matches!(self, Self::Rec | Self::Dub | Self::Drain)
    }

    /// Whether the loop is audible.
    #[must_use]
    pub const fn is_playing(self) -> bool {
        matches!(self, Self::Play | Self::Dub | Self::Drain)
    }
}

/// Where the record latency came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatencySource {
    /// Estimated from the device's ALSA ring capacities (the normal case).
    Geometry,
    /// An estimate, used while the geometry was not known yet.
    Estimate,
    /// A fixed value from the environment.
    Fixed,
    /// Seeded from a previous run.
    Seeded,
    /// Reported by the app's JUCE audio device at the callback boundary.
    Device,
}

impl LatencySource {
    /// The one-letter code shown in the view.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Geometry => b'B',
            Self::Estimate => b'E',
            Self::Fixed => b'F',
            Self::Seeded => b'S',
            Self::Device => b'J',
        }
    }

    /// Rebuild a source code, defaulting to [`LatencySource::Estimate`].
    #[must_use]
    pub const fn from_code(code: u8) -> Self {
        match code {
            b'B' => Self::Geometry,
            b'F' => Self::Fixed,
            b'S' => Self::Seeded,
            b'J' => Self::Device,
            _ => Self::Estimate,
        }
    }
}

/// One pad, laid out so the emulator test can read `state`, `len`, `rec` and
/// `lat` as four consecutive words through the exported `g_pad_stride`.
#[repr(C)]
pub struct Pad {
    state: AtomicI32,
    /// Closed loop length in frames.
    len: AtomicU32,
    /// Frames captured so far; written by the audio thread only.
    rec: AtomicU32,
    /// Latency applied to this take, in frames.
    lat: AtomicU32,
    /// Transport position the loop starts at.
    origin: AtomicU64,
    /// Frames still to drop at the start of a take.
    skip: AtomicU32,
    /// During a drain: whether the closed take was an overdub.
    touched: AtomicI32,
    /// Frames still to capture after the closing press.
    drain: AtomicU32,
}

impl Pad {
    const fn new() -> Self {
        Self {
            state: AtomicI32::new(State::Idle as i32),
            len: AtomicU32::new(0),
            rec: AtomicU32::new(0),
            lat: AtomicU32::new(0),
            origin: AtomicU64::new(0),
            skip: AtomicU32::new(0),
            touched: AtomicI32::new(0),
            drain: AtomicU32::new(0),
        }
    }

    fn state(&self) -> State {
        State::from_raw(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, state: State) {
        self.state.store(state.raw(), Ordering::Release);
    }
}

/// Read-only view of one pad for the paint hook.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct PadView {
    /// Published state; a draining take reads as [`State::Rec`] or
    /// [`State::Dub`] because it is still a take to the user.
    pub state: u8,
    /// Closed loop length in frames (zero while recording).
    pub len_frames: u32,
    /// Frames captured so far.
    pub rec_frames: u32,
    /// Playhead in frames, zero when the pad is not playing.
    pub pos: u32,
}

/// Read-only view of the whole engine for the paint hook.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Whether the fullscreen view is open.
    pub open: i32,
    /// Whether the sync pulse is running.
    pub sync: i32,
    /// Whether the metronome click is enabled.
    pub click: i32,
    /// Sync pulse period in frames.
    pub sync_frames: u32,
    /// Position within the pulse, or `u32::MAX` when the pulse is idle.
    pub pulse_phase: u32,
    /// Input level, 0..1.
    pub input: f32,
    /// Pad currently recording, or -1.
    pub recording_pad: i32,
    /// Latency applied to the last take.
    pub lat_frames: u32,
    /// One-letter latency source code.
    pub lat_src: u8,
    /// Status line, empty when there is nothing to say.
    pub message: [u8; MESSAGE_LEN],
    /// Per-pad view.
    pub pads: [PadView; PADS],
}

impl Default for View {
    fn default() -> Self {
        Self {
            open: 0,
            sync: 0,
            click: 0,
            sync_frames: 0,
            pulse_phase: u32::MAX,
            input: 0.0,
            recording_pad: -1,
            lat_frames: 0,
            lat_src: LatencySource::Estimate.code(),
            message: [0; MESSAGE_LEN],
            pads: [PadView::default(); PADS],
        }
    }
}

/// The pad array.  Exported under the name the emulator test reads.
#[unsafe(export_name = "g_pads")]
static PADS_ARRAY: [Pad; PADS] = [const { Pad::new() }; PADS];

/// Size of one [`Pad`], exported for the emulator test.
#[unsafe(export_name = "g_pad_stride")]
static PAD_STRIDE: u32 = size_of::<Pad>() as u32;

/// Whether the fullscreen view is open.  Exported for the emulator test.
#[unsafe(export_name = "g_open")]
static OPEN: AtomicI32 = AtomicI32::new(0);

/// Whether the metronome click is enabled.  Exported for the emulator test.
#[unsafe(export_name = "g_click")]
static CLICK: AtomicI32 = AtomicI32::new(0);

/// Sync pulse running.  Exported for the emulator test, like the pad array.
#[unsafe(export_name = "g_sync")]
static SYNC: AtomicI32 = AtomicI32::new(0);
/// Sync pulse period in frames.  Exported for the emulator test.
#[unsafe(export_name = "g_sync_frames")]
static SYNC_FRAMES: AtomicU32 = AtomicU32::new(0);
/// Transport position the current pulse started at.
static PULSE_EPOCH: AtomicU64 = AtomicU64::new(0);
/// Played frames since the view opened; the engine's clock.
static TRANSPORT: AtomicU64 = AtomicU64::new(0);
/// Bumped whenever the engine opens or closes.  A callback that started before
/// the reset checks it between blocks and exits instead of using reset state.
static GENERATION: AtomicU32 = AtomicU32::new(0);
/// Record latency in frames.
static LATENCY: AtomicU32 = AtomicU32::new(0);
/// Source of [`LATENCY`].
static LATENCY_SOURCE: AtomicU32 = AtomicU32::new(b'E' as u32);
/// Output-side device latency, in frames.  An overdub is shifted by the input
/// latency plus this, because the loop being played is this much ahead.
static LATENCY_OUT: AtomicU32 = AtomicU32::new(0);
/// Latency snapshotted for the last take.
static LATENCY_USED: AtomicU32 = AtomicU32::new(0);
/// Frames kept by the loop-only probe.
pub const LOOPTAP_FRAMES: usize = 8192;

/// The loop's contribution after the mod's master, before it is added to the
/// app's own output.  Unlike the device tap this holds nothing but the loops,
/// so it is the only place a level can be measured without the app's playhead
/// and monitor in the mix.  Exported as raw f32 bits for the acceptance harness,
/// which reads the symbol without going through the API.
#[unsafe(export_name = "g_looptap")]
pub static LOOPTAP: [AtomicU32; LOOPTAP_FRAMES] = [const { AtomicU32::new(0) }; LOOPTAP_FRAMES];

/// Write cursor of `g_looptap`.
#[unsafe(export_name = "g_looptap_w")]
pub static LOOPTAP_W: AtomicU32 = AtomicU32::new(0);

/// MAIN VOLUME, 0..127.  Exported so the acceptance harness can read the value
/// the mixer is actually using instead of inferring it from the audio.
#[unsafe(export_name = "g_looper_master")]
pub static MASTER: AtomicU32 = AtomicU32::new(127);
/// Mono input ring, as raw f32 bits.
static RING_BUFFER: [AtomicU32; RING as usize] = [const { AtomicU32::new(0) }; RING as usize];
/// Ring write cursor; single producer (the capture hook).
static RING_WRITE: AtomicU32 = AtomicU32::new(0);
/// Ring read cursor; single consumer (the playback hook).
static RING_READ: AtomicU32 = AtomicU32::new(0);
/// Pads with a take in progress.
static TAKE_COUNT: AtomicI32 = AtomicI32::new(0);
/// Per-pad start commands, claimed by the audio thread, which remains the
/// only writer of a pad's samples.
static START_REQUEST: [AtomicU8; PADS] = [const { AtomicU8::new(0) }; PADS];
const START_RECORD: u8 = 1;
const START_OVERDUB: u8 = 2;
/// Input position at each overdub request, before capture resumes.
static OVERDUB_CUT: [AtomicU32; PADS] = [const { AtomicU32::new(0) }; PADS];
/// Keeps capture alive while the audio thread claims an overdub request.
static CLAIMING_OVERDUB: AtomicBool = AtomicBool::new(false);
// Internal claim while the audio thread clears a pad for recording.
const STARTING: i32 = -1;
/// Input level as raw f32 bits.
static INPUT_LEVEL: AtomicU32 = AtomicU32::new(0);
/// The loop pool, mapped on open.
static POOL: AtomicPtr<f32> = AtomicPtr::new(core::ptr::null_mut());
/// Timestamp the status line expires at.
static MESSAGE_UNTIL: AtomicI64 = AtomicI64::new(0);
/// Status line bytes.
static MESSAGE: [AtomicU8; MESSAGE_LEN] = [const { AtomicU8::new(0) }; MESSAGE_LEN];

/// Operations on the process-wide looper engine.
///
/// The engine is a singleton (there is one capture stream and one playback
/// stream in the process), so these are associated functions over the module
/// state rather than methods on an instance.
#[derive(Clone, Copy, Debug)]
pub struct Engine;
impl Engine {
    /// Whether the fullscreen view is open.
    #[must_use]
    pub fn is_open() -> bool {
        OPEN.load(Ordering::Acquire) != 0
    }

    /// Open the view, mapping the loop pool on first use.
    ///
    /// Returns `false` only when the pool cannot be mapped; the already
    /// recorded loops survive a close/open cycle.
    pub fn open() -> bool {
        if Self::is_open() {
            return true;
        }
        if POOL.load(Ordering::Acquire).is_null() {
            let bytes = PADS * MAX_FRAMES * size_of::<f32>();
            // SAFETY: a private anonymous mapping owned by this process; the
            // pointer is stored once and never freed.
            let pool = unsafe {
                libc::mmap(
                    core::ptr::null_mut(),
                    bytes,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_PRIVATE | libc::MAP_ANONYMOUS,
                    -1,
                    0,
                )
            };
            if pool == libc::MAP_FAILED {
                return false;
            }
            POOL.store(pool.cast::<f32>(), Ordering::Release);
        }
        for pad in &PADS_ARRAY {
            let kept = pad.len.load(Ordering::Acquire) != 0;
            pad.set_state(if kept { State::Stop } else { State::Idle });
            pad.rec.store(0, Ordering::Release);
            pad.skip.store(0, Ordering::Release);
            pad.drain.store(0, Ordering::Release);
            pad.origin.store(0, Ordering::Release);
            pad.lat.store(0, Ordering::Release);
            pad.touched.store(0, Ordering::Release);
        }
        RING_WRITE.store(0, Ordering::Release);
        RING_READ.store(0, Ordering::Release);
        Self::clear_start_requests();
        TAKE_COUNT.store(0, Ordering::Release);
        INPUT_LEVEL.store(0, Ordering::Release);
        TRANSPORT.store(0, Ordering::Release);
        SYNC.store(0, Ordering::Release);
        SYNC_FRAMES.store(0, Ordering::Release);
        PULSE_EPOCH.store(0, Ordering::Release);
        // The metronome is off by default.
        CLICK.store(0, Ordering::Release);
        Self::clear_message();
        GENERATION.fetch_add(1, Ordering::AcqRel);
        OPEN.store(1, Ordering::Release);
        Self::notice("PAD: record   SHIFT+PAD: overdub");
        true
    }

    /// Close the view: stop everything, keep the recorded loops.
    pub fn close() {
        if !Self::is_open() {
            return;
        }
        Self::stop_all();
        Self::clear_start_requests();
        SYNC.store(0, Ordering::Release);
        INPUT_LEVEL.store(0, Ordering::Release);
        GENERATION.fetch_add(1, Ordering::AcqRel);
        OPEN.store(0, Ordering::Release);
    }

    /// Cancel every take a control thread requested but the audio thread has
    /// not started yet.
    fn clear_start_requests() {
        for request in &START_REQUEST {
            request.store(0, Ordering::Release);
        }
    }

    /// Stop every loop, keeping the material.
    pub fn stop_all() {
        for (index, pad) in PADS_ARRAY.iter().enumerate() {
            START_REQUEST[index].store(0, Ordering::Release);
            // Cancel an in-flight audio-thread buffer clear before it can
            // publish Rec. A successful recording start instead falls through.
            let cancelled = if pad.len.load(Ordering::Acquire) != 0 {
                State::Stop
            } else {
                State::Idle
            };
            let _ = pad.state.compare_exchange(
                STARTING,
                cancelled.raw(),
                Ordering::AcqRel,
                Ordering::Acquire,
            );
            let state = pad.state();
            if state.is_take() {
                if pad
                    .state
                    .compare_exchange(
                        state.raw(),
                        State::Stop.raw(),
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_ok()
                {
                    if state == State::Rec {
                        close_length(pad);
                    }
                    pad.drain.store(0, Ordering::Release);
                    pad.skip.store(0, Ordering::Release);
                    drop_take();
                }
            } else if state == State::Play {
                let _ = pad.state.compare_exchange(
                    State::Play.raw(),
                    State::Stop.raw(),
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
        }
    }

    /// Handle a pad press.  Returns `true` when the looper owns the event.
    pub fn pad(pad: usize, shift: bool, erase: bool) -> bool {
        if !Self::is_open() {
            return false;
        }
        let Some(entry) = PADS_ARRAY.get(pad) else {
            return true;
        };
        if erase {
            clear_pad(entry, pad);
            Self::notice("PAD CLEARED");
            return true;
        }
        let state = entry.state();
        if shift {
            match state {
                State::Play | State::Stop => request_overdub(pad),
                State::Dub | State::Rec | State::Drain => finish_pad(entry),
                State::Idle => request_record(pad),
            }
            return true;
        }
        match state {
            State::Idle => request_record(pad),
            State::Rec | State::Dub => finish_pad(entry),
            State::Play => entry.set_state(State::Stop),
            State::Stop => {
                // Synced loops keep their phase on the common transport when
                // restarted; unsynced loops start again at their first frame.
                if SYNC.load(Ordering::Acquire) == 0 {
                    entry
                        .origin
                        .store(TRANSPORT.load(Ordering::Acquire), Ordering::Release);
                }
                entry.set_state(State::Play);
            }
            // The take is already closing; nothing to do.
            State::Drain => {}
        }
        true
    }

    /// Handle a control-surface button while the view is open.
    ///
    /// Returns `true` when the event is consumed; while the view is open every
    /// button is consumed so the app underneath never sees it.
    pub fn button(code: u32, down: bool, shift: bool, erase: bool) -> bool {
        if !Self::is_open() {
            return false;
        }
        match code {
            // DATA: SHIFT+DATA closes the view.
            0x6f if down && shift => Self::close(),
            // STOP: stop every loop, keep the material.
            0x51 if down => {
                Self::stop_all();
                Self::notice("ALL STOPPED");
            }
            // LOOP: sync pulse; ERASE+LOOP clears everything; SHIFT+LOOP is the click.
            0x4a if down => {
                if erase {
                    for (index, pad) in PADS_ARRAY.iter().enumerate() {
                        clear_pad(pad, index);
                    }
                    SYNC.store(0, Ordering::Release);
                    SYNC_FRAMES.store(0, Ordering::Release);
                    Self::notice("ALL LOOPS CLEARED");
                } else if shift {
                    let on = CLICK.load(Ordering::Acquire) == 0;
                    CLICK.store(i32::from(on), Ordering::Release);
                    Self::notice(if on { "CLICK on" } else { "CLICK off" });
                } else {
                    Self::toggle_sync();
                }
            }
            _ => {}
        }
        true
    }

    /// MAIN VOLUME, 0..127.
    pub fn set_master(value: u32) {
        MASTER.store(value.min(127), Ordering::Release);
    }

    /// The current MAIN VOLUME.
    #[must_use]
    pub fn master() -> u32 {
        MASTER.load(Ordering::Acquire)
    }

    /// Set the record latency and where it came from.
    pub fn set_latency(frames: u32, source: LatencySource) {
        LATENCY.store(frames.min(LAT_MAX), Ordering::Release);
        LATENCY_SOURCE.store(u32::from(source.code()), Ordering::Release);
    }

    /// Set the output-side device latency used to place overdubs.
    pub fn set_output_latency(frames: u32) {
        LATENCY_OUT.store(frames.min(LAT_MAX), Ordering::Release);
    }

    /// Frames captured by the input hook but not consumed by playback yet.
    #[must_use]
    pub fn input_backlog() -> u32 {
        let write = RING_WRITE.load(Ordering::Acquire);
        let read = RING_READ.load(Ordering::Acquire);
        write.wrapping_sub(read)
    }

    /// Show a short status line in the view.
    pub fn notice(text: &str) {
        let bytes = text.as_bytes();
        let keep = bytes.len().min(MESSAGE_LEN - 1);
        for (index, slot) in MESSAGE.iter().enumerate() {
            slot.store(
                if index < keep { bytes[index] } else { 0 },
                Ordering::Relaxed,
            );
        }
        MESSAGE_UNTIL.store(now_ms() + 2_500, Ordering::Release);
    }

    /// Feed one planar S32 capture channel.
    pub fn input_s32(channel: &[i32]) {
        if !Self::is_open() || channel.is_empty() {
            return;
        }
        let generation = GENERATION.load(Ordering::Acquire);
        let mut block = [0.0_f32; BLOCK];
        for chunk in channel.chunks(BLOCK) {
            if GENERATION.load(Ordering::Acquire) != generation {
                return;
            }
            let mut peak = 0.0_f32;
            for (slot, &frame) in block.iter_mut().zip(chunk) {
                let value = s32_to_f32(frame);
                peak = peak.max(value.abs());
                *slot = value;
            }
            Self::input_block(&block[..chunk.len()], peak);
        }
    }

    /// Feed interleaved S32 capture; the first two channels carry the source.
    pub fn input_s32_interleaved(buffer: &[i32], channels: usize) {
        if !Self::is_open() || channels == 0 || buffer.is_empty() {
            return;
        }
        let generation = GENERATION.load(Ordering::Acquire);
        let frames = buffer.len() / channels;
        let mut block = [0.0_f32; BLOCK];
        let mut done = 0;
        while done < frames {
            if GENERATION.load(Ordering::Acquire) != generation {
                return;
            }
            let count = (frames - done).min(BLOCK);
            let mut peak = 0.0_f32;
            for i in 0..count {
                let frame = &buffer[(done + i) * channels..][..channels];
                let value = if channels >= 2 {
                    s32_pair_to_f32(frame[0], frame[1])
                } else {
                    s32_to_f32(frame[0])
                };
                peak = peak.max(value.abs());
                block[i] = value;
            }
            Self::input_block(&block[..count], peak);
            done += count;
        }
    }

    /// Mix the loops into the interleaved S32 block the device is about to play.
    ///
    /// This is the engine's clock: every frame mixed advances the transport.
    pub fn render_s32(buffer: &mut [i32], channels: usize) {
        if !Self::is_open() || channels == 0 || buffer.is_empty() {
            return;
        }
        let frames = buffer.len() / channels;
        let generation = GENERATION.load(Ordering::Acquire);
        start_requested(generation, TRANSPORT.load(Ordering::Acquire));
        let takes = TAKE_COUNT.load(Ordering::Acquire);
        let mut done = 0;
        let mut input = [0.0_f32; BLOCK];
        let mut add = [0.0_f32; BLOCK];
        while done < frames {
            // An open or close while this callback runs makes the reset state
            // meaningless; stop at the next block boundary.
            if GENERATION.load(Ordering::Acquire) != generation {
                return;
            }
            let count = (frames - done).min(BLOCK);
            add[..count].fill(0.0);
            let got = if takes != 0 {
                Self::drain_ring(&mut input, count)
            } else {
                0
            };
            input[got..count].fill(0.0);

            let transport = TRANSPORT.load(Ordering::Acquire);
            for (index, pad) in PADS_ARRAY.iter().enumerate() {
                let state = pad.state();
                if !state.is_playing() && state != State::Rec {
                    continue;
                }
                let Some(pointer) = pad_ptr(index) else {
                    continue;
                };
                // SAFETY: one slice per pad, held only for this iteration; the
                // pool is never freed and this thread is the only one that
                // writes a pad while it is being rendered.
                let samples = unsafe { core::slice::from_raw_parts_mut(pointer, MAX_FRAMES) };
                let len = pad.len.load(Ordering::Acquire);
                let origin = pad.origin.load(Ordering::Acquire);
                if state.is_playing() && len != 0 {
                    let base = transport.wrapping_sub(origin);
                    for (i, slot) in add.iter_mut().enumerate().take(count) {
                        let at = wrap_index(base.wrapping_add(i as u64), len);
                        // One fused multiply-add, exactly like the reference:
                        // the shipped build contracts `add += sample * gain`, so
                        // mixing two loops rounds once, not twice.
                        *slot = samples[at].mul_add(GAIN, *slot);
                    }
                }
                let touched = pad.touched.load(Ordering::Acquire) != 0;
                if state == State::Dub || (state == State::Drain && touched) {
                    let latency = if len != 0 {
                        pad.lat
                            .load(Ordering::Acquire)
                            .saturating_add(LATENCY_OUT.load(Ordering::Acquire))
                            % len
                    } else {
                        0
                    };
                    for (i, &value) in input.iter().enumerate().take(count) {
                        let pos = transport.wrapping_add(i as u64).wrapping_sub(origin);
                        dub_add(samples, len, latency, pos, value);
                    }
                } else if state == State::Rec || (state == State::Drain && !touched) {
                    if state == State::Drain {
                        pad.skip.store(0, Ordering::Release);
                    }
                    record(pad, samples, &input[..count]);
                }
                if state == State::Drain {
                    let drain = pad.drain.load(Ordering::Acquire);
                    let left = drain.saturating_sub(count as u32);
                    pad.drain.store(left, Ordering::Release);
                    // Only the take that is still draining may become playback:
                    // an erase or STOP in the meantime moved the state first, and
                    // resurrecting it here would leave a `Play` pad with no loop.
                    if left == 0
                        && pad
                            .state
                            .compare_exchange(
                                State::Drain.raw(),
                                State::Play.raw(),
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                    {
                        pad.skip.store(0, Ordering::Release);
                        drop_take();
                    }
                }
            }
            if SYNC.load(Ordering::Acquire) != 0 && CLICK.load(Ordering::Acquire) != 0 {
                mix_click(&mut add[..count], transport);
            }
            // Multiply by the reciprocal, like the reference: dividing by 127
            // would round differently and drift by one ulp per sample.
            let gain = f32::from(u16::try_from(Self::master()).unwrap_or(127)) * (1.0 / 127.0);
            for i in 0..count {
                let mixed = add[i] * gain;
                Self::tap_loop(mixed);
                let frame = &mut buffer[(done + i) * channels..][..channels];
                for sample in frame.iter_mut() {
                    let value = s32_to_f32(*sample) + mixed;
                    *sample = crate::sample::f32_to_s32(value);
                }
            }
            TRANSPORT.store(transport.wrapping_add(count as u64), Ordering::Release);
            done += count;
        }
    }

    /// Record the loop-only probe, wrapping at its capacity.
    ///
    /// Only the audio thread writes it; the acceptance harness reads the ring
    /// through `/proc/<pid>/mem`.  A plain relaxed store is enough for a single
    /// writer and a reader that only observes bytes.
    fn tap_loop(sample: f32) {
        let write = LOOPTAP_W.load(Ordering::Relaxed);
        let index = usize::try_from(write).unwrap_or(0) % LOOPTAP_FRAMES;
        LOOPTAP[index].store(sample.to_bits(), Ordering::Relaxed);
        LOOPTAP_W.store(write.wrapping_add(1), Ordering::Relaxed);
    }

    /// Fill `out` with input frames, keeping the ring shallow.
    fn drain_ring(out: &mut [f32], count: usize) -> usize {
        let mut read = RING_READ.load(Ordering::Acquire);
        let write = RING_WRITE.load(Ordering::Acquire);
        let mut available = write.wrapping_sub(read);
        if available > count as u32 * 4 {
            // The capture side is running ahead: drop the oldest frames so the
            // loop is heard as soon as possible instead of after a backlog.
            read = read.wrapping_add(available - count as u32 * 2);
            available = count as u32 * 2;
        }
        let got = usize::try_from(available.min(count as u32)).unwrap_or(0);
        for (i, slot) in out.iter_mut().take(got).enumerate() {
            let at = RING_BUFFER[wrap_index(u64::from(read.wrapping_add(i as u32)), RING)]
                .load(Ordering::Relaxed);
            *slot = f32::from_bits(at);
        }
        RING_READ.store(read.wrapping_add(got as u32), Ordering::Release);
        got
    }

    /// Copy a mono block into the ring.
    fn input_block(mono: &[f32], peak: f32) {
        if TAKE_COUNT.load(Ordering::Acquire) != 0
            || CLAIMING_OVERDUB.load(Ordering::Acquire)
            || START_REQUEST
                .iter()
                .any(|request| request.load(Ordering::Acquire) == START_OVERDUB)
        {
            let mut write = RING_WRITE.load(Ordering::Relaxed);
            for &value in mono {
                let read = RING_READ.load(Ordering::Relaxed);
                if write.wrapping_add(1).wrapping_sub(read) < RING {
                    RING_BUFFER[(write % RING) as usize].store(value.to_bits(), Ordering::Relaxed);
                    write = write.wrapping_add(1);
                } else {
                    break;
                }
            }
            RING_WRITE.store(write, Ordering::Release);
        }
        // Peak with a decay, so the meter falls back after the source stops.
        let previous = f32::from_bits(INPUT_LEVEL.load(Ordering::Relaxed));
        let mut level = previous * 0.9;
        if peak > level {
            level = peak;
        }
        INPUT_LEVEL.store(level.min(1.0).to_bits(), Ordering::Relaxed);
    }

    /// Snapshot the engine for the paint hook.  Returns `true` when the view is
    /// open and something can be drawn.
    #[must_use]
    pub fn snapshot(view: &mut View) -> bool {
        *view = View::default();
        if !Self::is_open() {
            return false;
        }
        view.open = 1;
        view.sync = i32::from(SYNC.load(Ordering::Acquire) != 0);
        view.click = CLICK.load(Ordering::Acquire);
        view.sync_frames = SYNC_FRAMES.load(Ordering::Acquire);
        view.pulse_phase = u32::MAX;
        if view.sync != 0 && view.sync_frames != 0 {
            let elapsed = TRANSPORT
                .load(Ordering::Acquire)
                .wrapping_sub(PULSE_EPOCH.load(Ordering::Acquire));
            view.pulse_phase = u32::try_from(wrap_index(elapsed, view.sync_frames)).unwrap_or(0);
        }
        view.input = f32::from_bits(INPUT_LEVEL.load(Ordering::Acquire)).min(1.0);
        view.recording_pad = -1;
        view.lat_frames = LATENCY_USED.load(Ordering::Acquire);
        view.lat_src = u8::try_from(LATENCY_SOURCE.load(Ordering::Acquire)).unwrap_or(b'E');
        let transport = TRANSPORT.load(Ordering::Acquire);
        for (index, pad) in PADS_ARRAY.iter().enumerate() {
            let mut state = pad.state();
            // A draining take is still a take to the user.
            if state == State::Drain {
                state = if pad.touched.load(Ordering::Acquire) != 0 {
                    State::Dub
                } else {
                    State::Rec
                };
            }
            let len = pad.len.load(Ordering::Acquire);
            let slot = &mut view.pads[index];
            slot.state = state as u8;
            slot.len_frames = len;
            slot.rec_frames = pad.rec.load(Ordering::Acquire);
            if matches!(state, State::Play | State::Dub) && len != 0 {
                slot.pos = u32::try_from(wrap_index(
                    transport.wrapping_sub(pad.origin.load(Ordering::Acquire)),
                    len,
                ))
                .unwrap_or(0);
            }
            if matches!(state, State::Rec | State::Dub) && view.recording_pad < 0 {
                view.recording_pad = i32::try_from(index).unwrap_or(0);
            }
        }
        if MESSAGE[0].load(Ordering::Acquire) != 0
            && now_ms() < MESSAGE_UNTIL.load(Ordering::Acquire)
        {
            for (slot, byte) in view.message.iter_mut().zip(MESSAGE.iter()) {
                *slot = byte.load(Ordering::Acquire);
            }
        }
        true
    }

    /// Turn the sync pulse on with the length of the newest loop, or off.
    fn toggle_sync() {
        if SYNC.load(Ordering::Acquire) != 0 {
            SYNC.store(0, Ordering::Release);
            Self::notice("SYNC off");
            return;
        }
        let mut best = 0;
        let mut newest = 0;
        for pad in &PADS_ARRAY {
            let len = pad.len.load(Ordering::Acquire);
            let origin = pad.origin.load(Ordering::Acquire);
            if len != 0 && origin >= newest {
                newest = origin;
                best = len;
            }
        }
        if best == 0 {
            Self::notice("SYNC: record a loop first");
            return;
        }
        SYNC_FRAMES.store(best, Ordering::Release);
        PULSE_EPOCH.store(newest, Ordering::Release);
        SYNC.store(1, Ordering::Release);
        let millis = u64::from(best) * 1000 / u64::from(RATE);
        let mut text = TextLine::new();
        // Formatting into a fixed buffer cannot fail.
        let _ = write!(&mut text, "SYNC {:.2}s", millis as f64 / 1000.0);
        Self::notice(text.as_str());
    }

    fn clear_message() {
        for byte in &MESSAGE {
            byte.store(0, Ordering::Relaxed);
        }
        MESSAGE_UNTIL.store(0, Ordering::Relaxed);
    }
}

/// A fixed-capacity UTF-8 buffer for status messages, so formatting a status
/// line never allocates.
struct TextLine {
    bytes: [u8; MESSAGE_LEN],
    len: usize,
}

impl TextLine {
    const fn new() -> Self {
        Self {
            bytes: [0; MESSAGE_LEN],
            len: 0,
        }
    }

    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or_default()
    }
}

impl core::fmt::Write for TextLine {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        let room = self.bytes.len() - self.len;
        let mut take = text.len().min(room);
        // Never cut a UTF-8 sequence: a truncated message would make `as_str`
        // return an empty line instead of a readable prefix.
        while !text.is_char_boundary(take) {
            take -= 1;
        }
        self.bytes[self.len..self.len + take].copy_from_slice(&text.as_bytes()[..take]);
        self.len += take;
        Ok(())
    }
}

/// `index % length` without dividing by zero.
fn wrap_index(index: u64, length: u32) -> usize {
    let length = u64::from(length.max(1));
    usize::try_from(index % length).unwrap_or(0)
}

/// Start of one pad's samples, or `None` before the pool is mapped.
///
/// The caller turns this into a slice.  The pool is one `PADS * MAX_FRAMES`
/// mapping that is never freed, so the pointer stays valid for the life of the
/// process; what the caller has to guarantee is that only one slice over a given
/// pad exists at a time (the engine never holds two, and the audio thread only
/// touches the pad it is currently rendering).
fn pad_ptr(index: usize) -> Option<*mut f32> {
    let pool = POOL.load(Ordering::Acquire);
    if pool.is_null() || index >= PADS {
        return None;
    }
    // SAFETY: `index < PADS`, so the offset stays inside the mapping.
    Some(unsafe { pool.add(index * MAX_FRAMES) })
}

/// Give a finished take its musical length: the transport span, snapped to a
/// whole number of pulses while the sync pulse is running.
///
/// The loop starts at the press, not at a pulse: moving the origin would shift
/// the performance itself, which is worse than starting off the beat.
fn close_length(pad: &Pad) {
    let span = TRANSPORT
        .load(Ordering::Acquire)
        .wrapping_sub(pad.origin.load(Ordering::Acquire));
    let recorded = pad.rec.load(Ordering::Acquire);
    let span = u32::try_from(span).unwrap_or(u32::MAX);
    let len = quantize(if span != 0 { span } else { recorded });
    pad.len
        .store(if len != 0 { len } else { recorded }, Ordering::Release);
}

/// The loop pool's capacity in frames.
fn pool_limit() -> u32 {
    u32::try_from(MAX_FRAMES).unwrap_or(u32::MAX)
}

/// The length a closed take gets: the nearest sync multiple, and never longer
/// than the pool that holds the take.
fn quantize(frames: u32) -> u32 {
    let pulse = SYNC_FRAMES.load(Ordering::Acquire);
    if SYNC.load(Ordering::Acquire) == 0 || pulse == 0 || frames == 0 {
        // Without a pulse there is nothing to snap to, but the pool bound still
        // applies: a take may outlast the eight seconds it can hold.
        return frames.min(pool_limit());
    }
    snap(frames, pulse, pool_limit())
}

/// The snapping rule without the globals: round `frames` up to a whole `pulse`
/// when that drops at most an eighth of one, then reduce it to a length that
/// fits `max`.
///
/// The reduction steps down from the unrounded multiple, exactly like the
/// reference: a take that was rounded *up* over the pool loses that extra pulse
/// before it loses a whole one.
fn snap(frames: u32, pulse: u32, max: u32) -> u32 {
    let pulse = pulse.max(1);
    let mut multiple = frames.saturating_add(pulse / 2) / pulse;
    if multiple < 1 {
        multiple = 1;
    }
    let pulse64 = u64::from(pulse);
    let max64 = u64::from(max);
    let base = u64::from(multiple) * pulse64;
    let mut target = base;
    let frames64 = u64::from(frames);
    if target < frames64 && frames64 - target > pulse64 / u64::from(QUANTIZE_SLACK) {
        target += pulse64;
    }
    if target > max64 {
        let floor = (max64 / pulse64) * pulse64;
        target = if multiple > 1 && target > base && base <= max64 {
            base - pulse64
        } else {
            floor
        };
    }
    u32::try_from(target).unwrap_or(0)
}

/// One thread wins the transition out of `Rec`/`Dub`; the loser returns without
/// touching the take count.
fn finish_pad(pad: &Pad) {
    let state = pad.state();
    if state != State::Rec && state != State::Dub {
        return;
    }
    let latency = pad.lat.load(Ordering::Acquire);
    let next = if latency != 0 {
        State::Drain
    } else {
        State::Play
    };
    // Publish all drain metadata before the state. The audio thread only
    // consumes these fields after it observes Drain with an acquire load.
    if state == State::Rec {
        close_length(pad);
    }
    pad.touched
        .store(i32::from(state == State::Dub), Ordering::Release);
    pad.drain.store(latency, Ordering::Release);
    if pad
        .state
        .compare_exchange(state.raw(), next.raw(), Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    if latency == 0 {
        drop_take();
    }
}

/// Forget one take.
///
/// Saturating, so a defensive extra call can never erase another take's count:
/// the old `fetch_sub` followed by a separate `store(0)` could zero a count a
/// concurrent `start_record` had just published.
fn drop_take() {
    let _ = TAKE_COUNT.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        (count > 0).then_some(count - 1)
    });
}

/// Clear one pad completely.
fn clear_pad(pad: &Pad, index: usize) {
    if let Some(request) = START_REQUEST.get(index) {
        request.store(0, Ordering::Release);
    }
    // Claim whatever state is current. In particular, this cancels a start
    // being prepared by the audio thread before it can publish a new take.
    let previous = State::from_raw(pad.state.swap(State::Idle.raw(), Ordering::AcqRel));
    if previous.is_take() {
        drop_take();
    }
    pad.len.store(0, Ordering::Release);
    pad.rec.store(0, Ordering::Release);
    pad.skip.store(0, Ordering::Release);
    pad.drain.store(0, Ordering::Release);
    pad.origin.store(0, Ordering::Release);
    pad.lat.store(0, Ordering::Release);
}

/// Ask the audio thread to start a fresh take on an idle pad.
///
/// The control thread must not touch the pad's samples: the audio thread can
/// still be rendering the previous take when the press arrives, so the clear and
/// the state transition both happen in [`start_requested`], the single writer.
fn request_record(index: usize) {
    if let Some(request) = START_REQUEST.get(index) {
        request.store(START_RECORD, Ordering::Release);
    }
}

/// Start every take a control thread requested, on the audio thread.
fn start_requested(generation: u32, transport: u64) {
    for (index, request) in START_REQUEST.iter().enumerate() {
        let requested = request.load(Ordering::Acquire);
        if requested == 0 || requested & 0x80 != 0 {
            continue;
        }
        // Capture remains enabled while an overdub is claimed by the audio thread.
        CLAIMING_OVERDUB.store(requested == START_OVERDUB, Ordering::Release);
        let claimed = requested | 0x80;
        if request
            .compare_exchange(requested, claimed, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            CLAIMING_OVERDUB.store(false, Ordering::Release);
            continue;
        }
        if GENERATION.load(Ordering::Acquire) != generation || !Engine::is_open() {
            let _ = request.compare_exchange(claimed, 0, Ordering::AcqRel, Ordering::Acquire);
            CLAIMING_OVERDUB.store(false, Ordering::Release);
            return;
        }
        let pad = &PADS_ARRAY[index];
        if requested == START_OVERDUB {
            start_overdub(
                pad,
                request,
                claimed,
                OVERDUB_CUT[index].load(Ordering::Acquire),
            );
            CLAIMING_OVERDUB.store(false, Ordering::Release);
            continue;
        }
        CLAIMING_OVERDUB.store(false, Ordering::Release);
        let Some(pointer) = pad_ptr(index) else {
            let _ = request.compare_exchange(claimed, 0, Ordering::AcqRel, Ordering::Acquire);
            continue;
        };
        if !claim_start(pad, request, claimed, State::Idle) {
            continue;
        }
        // Silence the previous take before the new state is visible.  Doing it
        // here is what makes the clear race-free: this is the only thread that
        // ever writes a pad's samples.
        // SAFETY: the audio thread owns every pad buffer, the pool is never
        // freed, and `index < PADS` keeps the slice inside the mapping.
        unsafe { core::slice::from_raw_parts_mut(pointer, MAX_FRAMES) }.fill(0.0);
        pad.len.store(0, Ordering::Release);
        pad.rec.store(0, Ordering::Release);
        pad.skip.store(0, Ordering::Release);
        pad.drain.store(0, Ordering::Release);
        pad.touched.store(0, Ordering::Release);
        pad.origin.store(transport, Ordering::Release);
        let latency = LATENCY.load(Ordering::Acquire).min(LAT_MAX);
        pad.lat.store(latency, Ordering::Release);
        LATENCY_USED.store(latency, Ordering::Release);
        pad.skip.store(latency, Ordering::Release);
        if TAKE_COUNT.load(Ordering::Acquire) == 0 {
            flush_input();
        }
        TAKE_COUNT.fetch_add(1, Ordering::AcqRel);
        if pad
            .state
            .compare_exchange(
                STARTING,
                State::Rec.raw(),
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_err()
        {
            drop_take();
        }
    }
}

/// Ask the audio thread to start an overdub, keeping ring reads on one thread.
fn request_overdub(index: usize) {
    if let Some(request) = START_REQUEST.get(index) {
        OVERDUB_CUT[index].store(RING_WRITE.load(Ordering::Acquire), Ordering::Release);
        request.store(START_OVERDUB, Ordering::Release);
    }
}

/// Claim a pad and its request together, so cancellation cannot be lost
/// between the request being read and the pad entering STARTING.
fn claim_start(pad: &Pad, request: &AtomicU8, claimed: u8, previous: State) -> bool {
    if pad
        .state
        .compare_exchange(
            previous.raw(),
            STARTING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        let _ = request.compare_exchange(claimed, 0, Ordering::AcqRel, Ordering::Acquire);
        return false;
    }
    if request
        .compare_exchange(claimed, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        return true;
    }
    let _ = pad.state.compare_exchange(
        STARTING,
        previous.raw(),
        Ordering::AcqRel,
        Ordering::Acquire,
    );
    false
}

/// Start an overdub on the audio thread.
fn start_overdub(pad: &Pad, request: &AtomicU8, claimed: u8, cut: u32) {
    let state = pad.state();
    if !matches!(state, State::Play | State::Stop) || pad.len.load(Ordering::Acquire) == 0 {
        let _ = request.compare_exchange(claimed, 0, Ordering::AcqRel, Ordering::Acquire);
        return;
    }
    if !claim_start(pad, request, claimed, state) {
        return;
    }
    let latency = LATENCY.load(Ordering::Acquire).min(LAT_MAX);
    pad.lat.store(latency, Ordering::Release);
    LATENCY_USED.store(latency, Ordering::Release);
    pad.drain.store(0, Ordering::Release);
    pad.skip.store(0, Ordering::Release);
    if TAKE_COUNT.load(Ordering::Acquire) == 0 {
        RING_READ.store(cut, Ordering::Release);
    }
    TAKE_COUNT.fetch_add(1, Ordering::AcqRel);
    if pad
        .state
        .compare_exchange(
            STARTING,
            State::Dub.raw(),
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        drop_take();
    }
}

/// A new take starts from the newest input frame: anything older in the ring
/// belongs to the previous take and would play as a ghost at the start.
fn flush_input() {
    let write = RING_WRITE.load(Ordering::Acquire);
    RING_READ.store(write, Ordering::Release);
}

/// Append one block of input to a recording pad.
fn record(pad: &Pad, samples: &mut [f32], input: &[f32]) {
    let mut written = pad.rec.load(Ordering::Acquire);
    let mut skip = pad.skip.load(Ordering::Acquire);
    for &value in input {
        if skip != 0 {
            skip -= 1;
            continue;
        }
        let Some(slot) = samples.get_mut(written as usize) else {
            break;
        };
        *slot = value;
        written += 1;
    }
    pad.skip.store(skip, Ordering::Release);
    pad.rec.store(written, Ordering::Release);
}

/// Add one sample to a loop at the overdub position, latency shifted, wrapped.
///
/// The input is `latency` frames old, so it belongs at `(pos - latency) mod
/// len`; a position that lands before the loop start wraps to the end of the
/// loop.  `latency` is always shorter than `len` (the caller reduces it), so the
/// two arms below cover every position.
fn dub_add(samples: &mut [f32], len: u32, latency: u32, pos: u64, value: f32) {
    if len == 0 {
        return;
    }
    let latency = latency.min(len - 1) as usize;
    let target = wrap_index(pos, len);
    let shifted = if target >= latency {
        target - latency
    } else {
        target + len as usize - latency
    };
    let Some(slot) = samples.get_mut(shifted) else {
        return;
    };
    *slot = (*slot + value).clamp(-1.0, 1.0);
}

/// One metronome click sample.
fn click_sample(phase: u64) -> f32 {
    if phase >= CLICK_FRAMES {
        return 0.0;
    }
    let phase_f = u32::try_from(phase).unwrap_or(0) as f32;
    let envelope = (-phase_f / 70.0).exp();
    let phase = core::f32::consts::TAU * 1000.0 * phase_f / (RATE as f32);
    phase.sin() * envelope * 0.35
}

/// Mix the click onto the pulse grid.
fn mix_click(add: &mut [f32], transport: u64) {
    let pulse = SYNC_FRAMES.load(Ordering::Acquire);
    if pulse == 0 {
        return;
    }
    let epoch = PULSE_EPOCH.load(Ordering::Acquire);
    for (i, slot) in add.iter_mut().enumerate() {
        let phase = wrap_index(transport.wrapping_add(i as u64).wrapping_sub(epoch), pulse);
        *slot += click_sample(u64::try_from(phase).unwrap_or(0));
    }
}

/// The first `count` samples of one pad's loop buffer.  Only for tests: it hands
/// out the recorded material so a golden fixture can compare it with the
/// reference implementation's buffer.
#[cfg(test)]
#[must_use]
pub fn pad_samples_for_tests(index: usize, count: usize) -> Vec<f32> {
    let Some(pointer) = pad_ptr(index) else {
        return Vec::new();
    };
    // SAFETY: the pool is never freed and `count` is clamped to one pad.
    unsafe { core::slice::from_raw_parts(pointer, count.min(MAX_FRAMES)) }.to_vec()
}

/// Reset every piece of engine state.  Only for tests: production code opens
/// and closes the view instead.
#[cfg(test)]
pub fn reset_for_tests() {
    Engine::close();
    for (index, pad) in PADS_ARRAY.iter().enumerate() {
        clear_pad(pad, index);
    }
    Engine::clear_start_requests();
    SYNC.store(0, Ordering::Release);
    SYNC_FRAMES.store(0, Ordering::Release);
    PULSE_EPOCH.store(0, Ordering::Release);
    TRANSPORT.store(0, Ordering::Release);
    LATENCY.store(0, Ordering::Release);
    LATENCY_OUT.store(0, Ordering::Release);
    LATENCY_USED.store(0, Ordering::Release);
    MASTER.store(127, Ordering::Release);
    RING_WRITE.store(0, Ordering::Release);
    RING_READ.store(0, Ordering::Release);
    TAKE_COUNT.store(0, Ordering::Release);
    INPUT_LEVEL.store(0, Ordering::Release);
    Engine::clear_message();
}

#[cfg(test)]
mod tests {
    use super::{MAX_FRAMES, QUANTIZE_SLACK, dub_add, pool_limit, quantize, snap};

    #[test]
    fn cancelled_start_cannot_claim_or_publish_a_pad() {
        use super::{Pad, START_RECORD, STARTING, State, claim_start};
        use core::sync::atomic::{AtomicU8, Ordering};
        let pad = Pad::new();
        let request = AtomicU8::new(0);
        let claimed = START_RECORD | 0x80;
        assert!(!claim_start(&pad, &request, claimed, State::Idle));
        assert_eq!(pad.state(), State::Idle);
        request.store(claimed, Ordering::Release);
        assert!(claim_start(&pad, &request, claimed, State::Idle));
        // STOP/ERASE cancels the claim before the audio thread can publish Rec.
        pad.state.store(State::Idle.raw(), Ordering::Release);
        assert!(
            pad.state
                .compare_exchange(
                    STARTING,
                    State::Rec.raw(),
                    Ordering::AcqRel,
                    Ordering::Acquire
                )
                .is_err()
        );
        assert_eq!(pad.state(), State::Idle);
    }

    #[test]
    fn overdub_drain_publishes_its_metadata_with_the_state() {
        use super::{Pad, State, finish_pad};
        use core::sync::atomic::Ordering;
        let pad = Pad::new();
        pad.lat.store(321, Ordering::Release);
        pad.set_state(State::Dub);
        finish_pad(&pad);
        assert_eq!(pad.state(), State::Drain);
        assert_eq!(pad.touched.load(Ordering::Acquire), 1);
        assert_eq!(pad.drain.load(Ordering::Acquire), 321);
    }

    /// The original step-by-step rule, kept as the oracle for [`snap`].
    fn reference(frames: u32, pulse: u32, max: u32) -> u32 {
        let max = u64::from(max);
        let mut multiple = frames.saturating_add(pulse / 2) / pulse;
        if multiple < 1 {
            multiple = 1;
        }
        let mut target = u64::from(multiple) * u64::from(pulse);
        let frames = u64::from(frames);
        if target < frames && frames - target > u64::from(pulse / QUANTIZE_SLACK) {
            target += u64::from(pulse);
        }
        while target > max && multiple > 1 {
            multiple -= 1;
            target = u64::from(multiple) * u64::from(pulse);
        }
        if target > max {
            target = (max / u64::from(pulse)) * u64::from(pulse);
        }
        u32::try_from(target).unwrap_or(0)
    }

    #[test]
    fn snapping_matches_the_reference_rule() {
        for pulse in [1_u32, 7, 100, 1_000, 44_100] {
            for frames in [
                0_u32, 1, 3, 50, 99, 100, 101, 999, 1_000, 1_001, 2_500, 44_100, 352_800, 352_801,
                500_000,
            ] {
                for max in [0_u32, 1, 999, 1_000, 2_500, 352_800] {
                    assert_eq!(
                        snap(frames, pulse, max),
                        reference(frames, pulse, max),
                        "frames {frames} pulse {pulse} max {max}"
                    );
                }
            }
        }
    }

    #[test]
    fn a_loop_is_never_longer_than_the_pool() {
        let limit = pool_limit();
        assert_eq!(limit, u32::try_from(MAX_FRAMES).unwrap_or(u32::MAX));
        assert_eq!(quantize(0), 0);
        assert!(quantize(limit) <= limit);
        assert!(
            quantize(limit.saturating_add(1)) <= limit,
            "cut to the pool"
        );
    }

    #[test]
    fn an_overdub_wraps_to_the_end_of_the_loop() {
        let mut samples = [0.0_f32; 10];
        // pos 10 in a 10-frame loop is the next pass's first frame; with a
        // latency of 3 it belongs at frame 7.
        dub_add(&mut samples, 10, 3, 10, 1.0);
        assert_eq!(samples.iter().position(|value| *value > 0.0), Some(7));
        assert!(samples[..7].iter().all(|value| *value == 0.0));
        // A target before the latency wraps to the end instead of being dropped:
        // target 1 - latency 3 is frame 8.
        dub_add(&mut samples, 10, 3, 11, 1.0);
        assert!(samples[8] > 0.0, "the seam must be overdubbed");
        // target == latency is frame 0.
        dub_add(&mut samples, 10, 3, 13, 1.0);
        assert!(samples[0] > 0.0);
        // The wrap keeps every pass writing exactly one frame.
        let mut written = [0_u8; 10];
        for pos in 0..30 {
            let mut one = [0.0_f32; 10];
            dub_add(&mut one, 10, 3, pos, 1.0);
            let index = one
                .iter()
                .position(|value| *value > 0.0)
                .expect("writes one frame");
            written[index] += 1;
        }
        assert!(
            written.iter().all(|count| *count == 3),
            "every frame is written once per pass: {written:?}"
        );
    }
}
