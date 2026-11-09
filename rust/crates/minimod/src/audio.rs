//! Reading the device's stream geometry off the app's audio object.
//!
//! The app keeps the channel count of each stream in its own object, sometimes
//! in a second field when the first is negative.  Getting this wrong would mix
//! the wrong number of channels, so the fallback to stereo is explicit.

use crate::firmware;

/// `[self + 0xd8]` is the output channel count, `+0xe4` its alternative.
const OUTPUT_CHANNELS: usize = 0xd8;
const OUTPUT_ALT: usize = 0xe4;
/// `[self + 0xd4]` is the input channel count, `+0xe0` its alternative.
const INPUT_CHANNELS: usize = 0xd4;
const INPUT_ALT: usize = 0xe0;
/// The device never has more channels than this.
const CHANNEL_LIMIT: i32 = 8;

/// The channel count of one side of the app's audio object.
#[must_use]
pub fn channels(self_address: usize, input_side: bool) -> usize {
    let read = |offset: usize| -> i32 {
        firmware::read_u32(self_address.wrapping_add(offset)).map_or(-1, u32::cast_signed)
    };
    let mut count = if input_side {
        let primary = read(INPUT_CHANNELS);
        if primary < 0 {
            read(INPUT_ALT)
        } else {
            primary
        }
    } else {
        let primary = read(OUTPUT_CHANNELS);
        if primary < 0 {
            read(OUTPUT_ALT)
        } else {
            primary
        }
    };
    if !(1..=CHANNEL_LIMIT).contains(&count) {
        count = 2;
    }
    usize::try_from(count).unwrap_or(2)
}

/// The channel count of one side, for the C glue.
#[unsafe(no_mangle)]
pub extern "C" fn minimod_audio_channels(self_address: usize, input_side: i32) -> i32 {
    i32::try_from(channels(self_address, input_side != 0)).unwrap_or(2)
}

/// Frames the writer will ever see in one block.
pub const MAX_FRAMES: usize = 4096;
/// Channels the writer will ever see.
pub const MAX_CHANNELS: usize = 8;

/// The app's writer: closure, channel array, frames, offset.
pub type Writer = unsafe extern "C" fn(usize, usize, usize, i32);

/// The copy the mod hands to the app when it mixed something in.
///
/// Flat and heap-allocated on first use rather than a thread-local: a 128 KiB
/// thread-local lands in `.tbss` and pushed `.bss` outside the loader's last
/// segment, so the mod's own globals were addressed past the end of its image.
static SCRATCH: std::sync::Mutex<Option<Box<[f32]>>> = std::sync::Mutex::new(None);

/// Run `body` with the scratch buffer, allocating it on first use.
fn with_scratch<T>(body: impl FnOnce(&mut [f32]) -> T) -> Option<T> {
    let mut guard = SCRATCH.lock().ok()?;
    let buffer = guard.get_or_insert_with(|| vec![0.0_f32; MAX_FRAMES * MAX_CHANNELS].into());
    Some(body(buffer))
}

/// Mix the preview into the app's block and forward it.
///
/// Returns the preview's peak when it produced audio, so the caller can keep its
/// statistics without the writer carrying them.
///
/// # Safety
///
/// `input` must be an array of `channels(self_address, false)` writable channel
/// pointers, each valid for `frames` samples, and `orig` must be the writer the
/// hook displaced.
pub unsafe fn write(
    self_address: usize,
    input: *mut *mut f32,
    frames: usize,
    offset: i32,
    orig: Option<Writer>,
    preview: bool,
) -> Option<f32> {
    if !preview {
        // Clear ringing voices after the menu closes, then pass the block on.
        // SAFETY: an empty channel list mixes nothing.
        unsafe { minimod_dsp::bass::preview_mix(&mut [], 0, 44_100) };
        forward(orig, self_address, input, frames, offset);
        return None;
    }
    let count = channels(self_address, false);
    if input.is_null() || frames == 0 || frames > MAX_FRAMES || !(1..=MAX_CHANNELS).contains(&count)
    {
        forward(orig, self_address, input, frames, offset);
        return None;
    }
    let peak = with_scratch(|scratch| {
        let mut pointers: [*mut f32; MAX_CHANNELS] = [core::ptr::null_mut(); MAX_CHANNELS];
        for (index, slot) in pointers.iter_mut().enumerate().take(count) {
            // SAFETY: the caller promises each channel pointer is readable.
            let source = unsafe { *input.add(index) };
            if source.is_null() {
                return None;
            }
            let channel = &mut scratch[index * MAX_FRAMES..][..frames];
            // SAFETY: the source is readable for `frames` samples and the
            // destination is a slice of exactly that length.
            unsafe {
                core::ptr::copy_nonoverlapping(source, channel.as_mut_ptr(), frames);
            }
            *slot = channel.as_mut_ptr();
        }
        // SAFETY: every pointer in the array points at `frames` writable samples.
        let peak =
            unsafe { minimod_dsp::bass::preview_mix(&mut pointers[..count], frames, 44_100) };
        Some((pointers, peak))
    });
    let Some(Some((mut pointers, peak))) = peak else {
        forward(orig, self_address, input, frames, offset);
        return None;
    };
    forward(orig, self_address, pointers.as_mut_ptr(), frames, offset);
    (peak > 0.0).then_some(peak)
}

/// Forward a block to the app's own writer.
fn forward(
    orig: Option<Writer>,
    self_address: usize,
    channels: *mut *mut f32,
    frames: usize,
    offset: i32,
) {
    if let Some(writer) = orig {
        // SAFETY: the caller guarantees this is the displaced writer and the
        // block matches what the app gave it.
        unsafe { writer(self_address, channels as usize, frames, offset) };
    }
}

/// The audio writer, for the C glue.
///
/// # Safety
///
/// See [`write`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_audio_writer(
    self_address: usize,
    input: *mut *mut f32,
    frames: usize,
    offset: i32,
    orig: Option<Writer>,
    preview: i32,
    peak_out: *mut f32,
) -> i32 {
    // SAFETY: see the function contract.
    let peak = unsafe { write(self_address, input, frames, offset, orig, preview != 0) };
    match peak {
        Some(value) => {
            if !peak_out.is_null() {
                // SAFETY: the caller promises `peak_out` is writable.
                unsafe { *peak_out = value };
            }
            1
        }
        None => 0,
    }
}

/// How many times the app asked for audio, for the device tests.
#[unsafe(no_mangle)]
pub static mut g_audio_out_calls: u32 = 0;

/// How many blocks the bass preview produced.
#[unsafe(no_mangle)]
pub static mut g_bass_preview_blocks: u32 = 0;

/// The loudest block the preview produced, as bits.
#[unsafe(no_mangle)]
pub static mut g_bass_preview_peak_bits: u32 = 0;

/// Whether the bass preview is audible.
#[unsafe(no_mangle)]
pub static mut g_bass_preview_active: i32 = 0;

/// The component the page last painted.
#[unsafe(no_mangle)]
pub static mut g_paint_comp: usize = 0;

/// The app's own audio writer.
static mut ORIGINAL_WRITER: Option<Writer> = None;

/// Record the displaced writer, for the glue.
///
/// # Safety
///
/// The constructor calls this once, before any audio can arrive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_audio_writer_original(original: Option<Writer>) {
    // SAFETY: the caller's contract.
    unsafe { ORIGINAL_WRITER = original };
}

/// The audio writer: count the call, and track the preview's peak.
///
/// # Safety
///
/// The app calls this with its own buffers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_hook_audio_writer(
    self_address: usize,
    input: *mut *mut f32,
    frames: usize,
    offset: i32,
) {
    // SAFETY: the counters are statics in this module.
    unsafe { core::ptr::addr_of_mut!(g_audio_out_calls).write_volatile(g_audio_out_calls + 1) };
    // SAFETY: see above.
    let preview = i32::from(unsafe { g_bass_preview_active } != 0);
    let mut peak = 0.0_f32;
    // SAFETY: the original is the app's own writer.
    let original = unsafe { *core::ptr::addr_of!(ORIGINAL_WRITER) };
    // SAFETY: the caller passed the app's buffers.
    let wrote = unsafe {
        minimod_audio_writer(
            self_address,
            input,
            frames,
            offset,
            original,
            preview,
            core::ptr::from_mut(&mut peak),
        )
    };
    if wrote == 0 || peak <= 0.0 {
        return;
    }
    // SAFETY: the counters are statics in this module.
    unsafe {
        core::ptr::addr_of_mut!(g_bass_preview_blocks).write_volatile(g_bass_preview_blocks + 1);
        if peak > f32::from_bits(g_bass_preview_peak_bits) {
            core::ptr::addr_of_mut!(g_bass_preview_peak_bits).write_volatile(peak.to_bits());
        }
    }
}

/// The looper's repaint pump: a steady tick while the view is open.
///
/// # Safety
///
/// Called as a thread entry point.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_pump_thread(_arg: usize) -> usize {
    loop {
        std::thread::sleep(core::time::Duration::from_millis(60));
        if crate::looper_ffi::looper_is_open() == 0 {
            continue;
        }
        // SAFETY: the paint statics are the glue's.
        let painted = unsafe { g_paint_comp };
        let component = if painted != 0 {
            painted
        } else {
            crate::paint::screen()
        };
        if component != 0 {
            // SAFETY: the original is the app's own repaint.
            if let Some(original) = crate::glue::original_repaint() {
                // SAFETY: the app's own call.
                unsafe { original(component, 0, (2000_u64 << 32) | 0x7d0) };
            }
        }
    }
}

/// The PCM format code for signed 32-bit little-endian samples.
const FORMAT_S32_LE: i32 = 10;
/// The largest device buffer the mod will believe, in frames.
const DEVICE_MAX_FRAMES: u64 = 65536;
/// The largest device channel count the mod will believe.
const DEVICE_MAX_CHANNELS: u32 = 32;

/// The ALSA entry points, resolved once from the next object in the search order.
type HwMalloc = unsafe extern "C" fn(*mut *mut libc::c_void) -> i32;
type HwCurrent = unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_void) -> i32;
type HwBufsize = unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_ulong) -> i32;
type HwChannels = unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_uint) -> i32;
type HwFormat = unsafe extern "C" fn(*mut libc::c_void, *mut i32) -> i32;
type HwFree = unsafe extern "C" fn(*mut libc::c_void);

struct Alsa {
    malloc: HwMalloc,
    current: HwCurrent,
    bufsize: HwBufsize,
    channels: HwChannels,
    format: HwFormat,
    free: HwFree,
}

/// Resolve one symbol from the object after this one.
fn look_up<T: Copy>(name: &core::ffi::CStr) -> Option<T> {
    // SAFETY: the name is NUL-terminated and the search order is the loader's.
    let address = unsafe { libc::dlsym(libc::RTLD_NEXT, name.as_ptr()) };
    if address.is_null() {
        return None;
    }
    // SAFETY: the caller names the type the symbol actually has.
    Some(unsafe { core::mem::transmute_copy::<*mut libc::c_void, T>(&address) })
}

impl Alsa {
    /// Resolve every entry point, or nothing if one is missing.
    fn load() -> Option<&'static Self> {
        static SLOT: std::sync::OnceLock<Option<Alsa>> = std::sync::OnceLock::new();
        SLOT.get_or_init(|| {
            Some(Self {
                malloc: look_up(c"snd_pcm_hw_params_malloc")?,
                current: look_up(c"snd_pcm_hw_params_current")?,
                bufsize: look_up(c"snd_pcm_hw_params_get_buffer_size")?,
                channels: look_up(c"snd_pcm_hw_params_get_channels")?,
                format: look_up(c"snd_pcm_hw_params_get_format")?,
                free: look_up(c"snd_pcm_hw_params_free")?,
            })
        })
        .as_ref()
    }
}

/// The device stream's geometry: its buffer in frames, its channels and whether
/// it carries 32-bit samples.
///
/// # Safety
///
/// `pcm` must be the app's live PCM handle; the out pointers may be null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_stream_geometry(
    pcm: usize,
    channels: *mut u32,
    s32: *mut i32,
) -> u32 {
    let Some(alsa) = Alsa::load() else {
        return 0;
    };
    if pcm == 0 {
        return 0;
    }
    let mut params: *mut libc::c_void = core::ptr::null_mut();
    // SAFETY: the entry point allocates the parameters.
    if unsafe { (alsa.malloc)(core::ptr::from_mut(&mut params)) } < 0 || params.is_null() {
        return 0;
    }
    let mut frames = 0_u32;
    // SAFETY: the handle is live and `params` is ours.
    if unsafe { (alsa.current)(pcm as *mut libc::c_void, params) } >= 0 {
        let mut size = 0_u64;
        // SAFETY: see above.
        if unsafe { (alsa.bufsize)(params, core::ptr::from_mut(&mut size)) } >= 0
            && size < DEVICE_MAX_FRAMES
        {
            frames = u32::try_from(size).unwrap_or(0);
        }
        if !channels.is_null() {
            let mut count = 0_u32;
            // SAFETY: see above.
            if unsafe { (alsa.channels)(params, core::ptr::from_mut(&mut count)) } >= 0
                && count > 0
                && count <= DEVICE_MAX_CHANNELS
            {
                // SAFETY: the caller passed a writable slot.
                unsafe { *channels = count };
            }
        }
        if !s32.is_null() {
            let mut format = -1;
            // SAFETY: see above.
            let known = unsafe { (alsa.format)(params, core::ptr::from_mut(&mut format)) } >= 0;
            // SAFETY: the caller passed a writable slot.
            unsafe { *s32 = i32::from(known && format == FORMAT_S32_LE) };
        }
    }
    // SAFETY: the parameters came from the malloc above.
    unsafe { (alsa.free)(params) };
    frames
}

/// Record the app's audio writer at install time.
///
/// # Safety
///
/// Called once, before any audio can arrive.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn minimod_audio_writer_original_now(original: usize) {
    // SAFETY: the caller's contract.
    unsafe { ORIGINAL_WRITER = crate::input::from_address(original) };
}

/// Frames the output tap keeps: two seconds at the device rate.
pub const OUTTAP_FRAMES: usize = 44100 * 2;

/// The last output delay the device reported, in frames.
#[unsafe(no_mangle)]
pub static mut g_out_delay: u32 = 0;
/// The same, smoothed, which is what a take uses.
#[unsafe(no_mangle)]
pub static mut g_out_delay_avg: u32 = 0;
/// The loudest sample written since the last reset, as bits.
#[unsafe(no_mangle)]
pub static mut g_out_peak_bits: u32 = 0;
/// How many buffers have been written.
#[unsafe(no_mangle)]
pub static mut g_out_write_calls: u32 = 0;
/// The output tap: the last two seconds, channel zero.
#[unsafe(no_mangle)]
pub static mut g_outtap: [f32; OUTTAP_FRAMES] = [0.0; OUTTAP_FRAMES];
/// Where the tap is writing.
#[unsafe(no_mangle)]
pub static mut g_outtap_w: u32 = 0;
/// Whether the device rings need re-reading.
#[unsafe(no_mangle)]
pub static mut g_buf_dirty: i32 = 0;
/// The output stream's channel count.
#[unsafe(no_mangle)]
pub static mut g_out_ch: u32 = 0;
/// Whether the output stream carries 32-bit samples.
#[unsafe(no_mangle)]
pub static mut g_out_s32: i32 = 0;

/// The app's own `snd_pcm_writei`.
static mut REAL_WRITEI: Option<unsafe extern "C" fn(usize, *const u8, u64) -> i64> = None;
/// The app's own `snd_pcm_delay`.
static mut REAL_DELAY: Option<unsafe extern "C" fn(usize, *mut i64) -> i64> = None;

/// The output tap: measure the device's latency and record what is played.
///
/// # Safety
///
/// `buffer` must hold `size` bytes, as the app's own writer requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_writei(pcm: usize, buffer: *const i32, size: u64) -> i64 {
    // SAFETY: the static is this module's.
    let mut real = unsafe { *core::ptr::addr_of!(REAL_WRITEI) };
    if real.is_none() {
        real = look_up(c"snd_pcm_writei");
        // SAFETY: written once.
        unsafe { REAL_WRITEI = real };
    }
    let Some(write) = real else {
        return -1;
    };
    let looper = crate::looper_ffi::looper_is_open() != 0;
    // The rings are only readable once the stream exists, so this is retried
    // until both directions report one.
    // SAFETY: the statics are this module's.
    let stale = unsafe { g_buf_dirty } != 0;
    if looper && (crate::glue::buffer_out() == 0 || stale) {
        // SAFETY: the statics are this module's.
        unsafe { refresh_rings(pcm) };
    }
    let channels = unsafe { g_out_ch };
    if looper && !buffer.is_null() && size != 0 && channels != 0 && unsafe { g_out_s32 } != 0 {
        // SAFETY: the buffer is the device's own output.
        unsafe {
            crate::looper_ffi::looper_render_s32(
                buffer.cast_mut(),
                channels,
                u32::try_from(size).unwrap_or(0),
            );
        }
    }
    if looper {
        // SAFETY: this module's statics.
        unsafe { measure_latency(pcm, size) };
    }
    // SAFETY: the app's own writer takes these.
    let written = unsafe { write(pcm, buffer.cast::<u8>(), size) };
    if !looper || written <= 0 || buffer.is_null() {
        return written;
    }
    // SAFETY: the statics are this module's and the buffer is the device's.
    unsafe { record_output(buffer, written, channels) };
    written
}

/// Read the device rings' geometry.
///
/// # Safety
///
/// `pcm` is the app's live handle.
unsafe fn refresh_rings(pcm: usize) {
    let mut channels = 0_u32;
    let mut s32 = 0_i32;
    // SAFETY: the geometry reads the stream's own parameters.
    let frames = unsafe {
        minimod_stream_geometry(
            pcm,
            core::ptr::from_mut(&mut channels),
            core::ptr::from_mut(&mut s32),
        )
    };
    // SAFETY: this module's statics and the glue's ring sizes.
    unsafe {
        crate::glue::set_buffer_out(frames);
        if channels != 0 {
            g_out_ch = channels;
            g_out_s32 = s32;
        }
        if frames != 0 && crate::glue::buffer_in() != 0 {
            g_buf_dirty = 0;
        }
    }
}

/// Measure the device's latency, the way the firmware's own sampler sees it.
///
/// # Safety
///
/// The statics are this module's.
unsafe fn measure_latency(pcm: usize, size: u64) {
    // SAFETY: this module's statics.
    unsafe {
        let mut real = *core::ptr::addr_of!(REAL_DELAY);
        if real.is_none() {
            real = look_up(c"snd_pcm_delay");
            REAL_DELAY = real;
        }
        if let Some(delay_of) = real {
            let mut delay = 0_i64;
            if delay_of(pcm, core::ptr::from_mut(&mut delay)) == 0 && delay >= 0 {
                g_out_delay = u32::try_from(delay).unwrap_or(0);
                let average = g_out_delay_avg;
                g_out_delay_avg = if average == 0 {
                    g_out_delay
                } else {
                    (average * 7 + g_out_delay) / 8
                };
            }
        }
        let fixed = crate::glue::fixed_latency();
        let (total, source) = if fixed >= 0 {
            (u32::try_from(fixed).unwrap_or(0), b'F')
        } else {
            let out = crate::glue::buffer_out();
            let input = crate::glue::buffer_in();
            let (measured, source) = if out > 0 && input > 0 {
                (u64::from(out) + u64::from(input), b'B')
            } else {
                (
                    u64::from(g_out_delay_avg)
                        + size
                        + u64::from(crate::glue::input_delay())
                        + u64::from(crate::looper_ffi::looper_input_backlog()),
                    b'E',
                )
            };
            let extra = crate::glue::extra_latency();
            (
                u32::try_from(measured.saturating_add(u64::from(extra)).min(16384))
                    .unwrap_or(16384),
                source,
            )
        };
        crate::looper_ffi::looper_set_latency_ex(total, i8::try_from(source).unwrap_or(0x45));
    }
}

/// Record what was written: the peak and the tap.
///
/// # Safety
///
/// `buffer` holds `written * channels` samples of 32-bit audio.
unsafe fn record_output(buffer: *const i32, written: i64, channels: u32) {
    const SCALE: f32 = 1.0 / 2_147_483_648.0;
    // SAFETY: this module's statics and the device's buffer.
    unsafe {
        g_out_write_calls = g_out_write_calls.wrapping_add(1);
        let count = usize::try_from(written).unwrap_or(0) * channels as usize;
        let mut peak = 0.0_f32;
        for index in 0..count {
            let value = (*buffer.add(index) as f32) * SCALE;
            peak = peak.max(value.abs());
        }
        if peak > f32::from_bits(g_out_peak_bits) {
            g_out_peak_bits = peak.to_bits();
        }
        let mut write = g_outtap_w;
        let frames = usize::try_from(written).unwrap_or(0);
        let tap = core::ptr::addr_of_mut!(g_outtap).cast::<f32>();
        for index in 0..frames {
            let slot = write as usize % OUTTAP_FRAMES;
            *tap.add(slot) = (*buffer.add(index * channels as usize) as f32) * SCALE;
            write = write.wrapping_add(1);
        }
        g_outtap_w = write;
    }
}

/// How many times the app asked for input audio.
#[unsafe(no_mangle)]
pub static mut g_audio_in_calls: u32 = 0;
/// The input stream's channel count.
#[unsafe(no_mangle)]
pub static mut g_in_ch: u32 = 0;
/// Whether the input stream carries 32-bit samples.
#[unsafe(no_mangle)]
pub static mut g_in_s32: i32 = 0;
/// The input ring's size in frames, as last read.
#[unsafe(no_mangle)]
pub static mut g_in_delay: u32 = 0;
/// The input ring's size in frames.
#[unsafe(no_mangle)]
pub static mut g_buf_in: u32 = 0;

/// The app's own `snd_pcm_readi`.
static mut REAL_READI: Option<unsafe extern "C" fn(usize, *mut u8, u64) -> i64> = None;
/// The app's own `snd_pcm_avail`.
static mut REAL_AVAIL: Option<unsafe extern "C" fn(usize) -> i64> = None;

/// The input tap: measure how much is waiting and feed the looper what arrived.
///
/// # Safety
///
/// `buffer` must hold `size` bytes, as the app's own reader requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_readi(pcm: usize, buffer: *mut i32, size: u64) -> i64 {
    // SAFETY: the static is this module's.
    let mut real = unsafe { *core::ptr::addr_of!(REAL_READI) };
    if real.is_none() {
        real = look_up(c"snd_pcm_readi");
        // SAFETY: written once.
        unsafe { REAL_READI = real };
    }
    let Some(capture) = real else {
        return -1;
    };
    let looper = crate::looper_ffi::looper_is_open() != 0;
    if looper {
        // SAFETY: this module's statics.
        unsafe { measure_input_delay(pcm) };
    }
    // The rings are only readable once the stream exists, so this is retried
    // until both directions report one.
    // SAFETY: the static is this module's.
    let stale = unsafe { g_buf_dirty } != 0;
    if crate::glue::buffer_in() == 0 || stale {
        // SAFETY: the statics are this module's.
        unsafe { refresh_input_ring(pcm) };
    }
    // SAFETY: the app's own reader takes these.
    let got = unsafe { capture(pcm, buffer.cast::<u8>(), size) };
    if got > 0 {
        // SAFETY: this module's statics.
        unsafe {
            g_audio_in_calls = g_audio_in_calls.wrapping_add(1);
        }
        // SAFETY: the buffer holds the frames just read.
        unsafe {
            if looper && !buffer.is_null() && g_in_ch != 0 && g_in_s32 != 0 {
                crate::looper_ffi::looper_input_s32_interleaved(
                    buffer,
                    g_in_ch,
                    u32::try_from(got).unwrap_or(0),
                );
            }
        }
    }
    got
}

/// How many frames the device has waiting, when the looper needs to know.
///
/// # Safety
///
/// `pcm` is the app's live handle.
unsafe fn measure_input_delay(pcm: usize) {
    // SAFETY: this module's statics.
    unsafe {
        let mut real = *core::ptr::addr_of!(REAL_AVAIL);
        if real.is_none() {
            real = look_up(c"snd_pcm_avail");
            REAL_AVAIL = real;
        }
        if let Some(avail_of) = real {
            let avail = avail_of(pcm);
            if avail > 0 {
                g_in_delay = u32::try_from(avail).unwrap_or(0);
            }
        }
    }
}

/// Read the input ring's geometry.
///
/// # Safety
///
/// `pcm` is the app's live handle.
unsafe fn refresh_input_ring(pcm: usize) {
    let mut channels = 0_u32;
    let mut s32 = 0_i32;
    // SAFETY: the geometry reads the stream's own parameters.
    let frames = unsafe {
        minimod_stream_geometry(
            pcm,
            core::ptr::from_mut(&mut channels),
            core::ptr::from_mut(&mut s32),
        )
    };
    // SAFETY: this module's statics and the glue's ring sizes.
    unsafe {
        crate::glue::set_buffer_in(frames);
        if channels != 0 {
            g_in_ch = channels;
            g_in_s32 = s32;
        }
        if frames != 0 && crate::glue::buffer_out() != 0 {
            g_buf_dirty = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CHANNEL_LIMIT, INPUT_ALT, INPUT_CHANNELS, OUTPUT_ALT, OUTPUT_CHANNELS, channels};

    #[test]
    fn the_offsets_are_the_documented_ones() {
        assert_eq!(OUTPUT_CHANNELS, 0xd8);
        assert_eq!(OUTPUT_ALT, 0xe4);
        assert_eq!(INPUT_CHANNELS, 0xd4);
        assert_eq!(INPUT_ALT, 0xe0);
        assert_eq!(CHANNEL_LIMIT, 8);
    }

    #[test]
    fn an_unreadable_object_falls_back_to_stereo() {
        // Address zero is never readable in the target process.
        assert_eq!(channels(0, false), 2);
        assert_eq!(channels(0, true), 2);
    }
}
