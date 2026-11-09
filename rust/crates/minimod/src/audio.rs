//! Device audio: the ALSA taps that feed the looper and the bass preview, the
//! stream geometry they read, and the looper's repaint pump.

/// How many blocks the bass preview produced.
pub static mut G_BASS_PREVIEW_BLOCKS: u32 = 0;

/// The loudest block the preview produced, as bits.
pub static mut G_BASS_PREVIEW_PEAK_BITS: u32 = 0;

/// Whether the bass preview is audible.
pub static mut G_BASS_PREVIEW_ACTIVE: i32 = 0;

/// The component the page last painted.
pub static mut G_PAINT_COMP: usize = 0;

/// The looper's repaint pump: a steady tick while the view is open.
///
/// # Safety
///
/// Called as a thread entry point.
#[must_use]
pub unsafe extern "C" fn minimod_pump_thread(_arg: usize) -> usize {
    loop {
        std::thread::sleep(core::time::Duration::from_millis(60));
        if crate::looper_ffi::looper_is_open() == 0 {
            continue;
        }
        // SAFETY: the paint statics are the glue's.
        let painted = unsafe { G_PAINT_COMP };
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

/// Frames the output tap keeps: two seconds at the device rate.
pub const OUTTAP_FRAMES: usize = 44100 * 2;

/// The last output delay the device reported, in frames.
pub static mut G_OUT_DELAY: u32 = 0;
/// The same, smoothed, which is what a take uses.
pub static mut G_OUT_DELAY_AVG: u32 = 0;
/// The loudest sample written since the last reset, as bits.
pub static mut G_OUT_PEAK_BITS: u32 = 0;
/// How many buffers have been written.
pub static mut G_OUT_WRITE_CALLS: u32 = 0;
/// The output tap: the last two seconds, channel zero.
#[unsafe(no_mangle)]
pub static mut g_outtap: [f32; OUTTAP_FRAMES] = [0.0; OUTTAP_FRAMES];
/// Where the tap is writing.
#[unsafe(no_mangle)]
pub static mut g_outtap_w: u32 = 0;
/// Whether the device rings need re-reading.
pub static mut G_BUF_DIRTY: i32 = 0;
/// The output stream's channel count.
pub static mut G_OUT_CH: u32 = 0;
/// Whether the output stream carries 32-bit samples.
pub static mut G_OUT_S32: i32 = 0;

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
    // SAFETY: the buffer is the device's own output, `size` frames of it.
    unsafe { mix_preview(pcm, buffer.cast_mut(), size) };
    let looper = crate::looper_ffi::looper_is_open() != 0;
    // The rings are only readable once the stream exists, so this is retried
    // until both directions report one.
    // SAFETY: the statics are this module's.
    let stale = unsafe { G_BUF_DIRTY } != 0;
    if looper && (crate::glue::buffer_out() == 0 || stale) {
        // SAFETY: the statics are this module's.
        unsafe { refresh_rings(pcm) };
    }
    let channels = unsafe { G_OUT_CH };
    if looper && !buffer.is_null() && size != 0 && channels != 0 && unsafe { G_OUT_S32 } != 0 {
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
            G_OUT_CH = channels;
            G_OUT_S32 = s32;
        }
        if frames != 0 && crate::glue::buffer_in() != 0 {
            G_BUF_DIRTY = 0;
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
                G_OUT_DELAY = u32::try_from(delay).unwrap_or(0);
                let average = G_OUT_DELAY_AVG;
                G_OUT_DELAY_AVG = if average == 0 {
                    G_OUT_DELAY
                } else {
                    (average * 7 + G_OUT_DELAY) / 8
                };
            }
        }
        let out = crate::glue::buffer_out();
        let input = crate::glue::buffer_in();
        let (measured, source) = if out > 0 && input > 0 {
            (u64::from(out) + u64::from(input), b'B')
        } else {
            (
                u64::from(G_OUT_DELAY_AVG)
                    + size
                    + u64::from(crate::glue::input_delay())
                    + u64::from(crate::looper_ffi::looper_input_backlog()),
                b'E',
            )
        };
        let extra = crate::glue::extra_latency();
        let total =
            u32::try_from(measured.saturating_add(u64::from(extra)).min(16384)).unwrap_or(16384);
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
        G_OUT_WRITE_CALLS = G_OUT_WRITE_CALLS.wrapping_add(1);
        let count = usize::try_from(written).unwrap_or(0) * channels as usize;
        let mut peak = 0.0_f32;
        for index in 0..count {
            let value = (*buffer.add(index) as f32) * SCALE;
            peak = peak.max(value.abs());
        }
        if peak > f32::from_bits(G_OUT_PEAK_BITS) {
            G_OUT_PEAK_BITS = peak.to_bits();
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
pub static mut G_AUDIO_IN_CALLS: u32 = 0;
/// The input stream's channel count.
pub static mut G_IN_CH: u32 = 0;
/// Whether the input stream carries 32-bit samples.
pub static mut G_IN_S32: i32 = 0;
/// The input ring's size in frames, as last read.
pub static mut G_IN_DELAY: u32 = 0;
/// The input ring's size in frames.
#[unsafe(no_mangle)]
pub static mut g_buf_in: u32 = 0;

/// The component the page last painted, or zero.
#[must_use]
pub fn painted() -> usize {
    // SAFETY: only the UI thread writes it.
    unsafe { G_PAINT_COMP }
}

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
    let stale = unsafe { G_BUF_DIRTY } != 0;
    if crate::glue::buffer_in() == 0 || stale {
        // SAFETY: the statics are this module's.
        unsafe { refresh_input_ring(pcm) };
    }
    // SAFETY: the app's own reader takes these.
    let got = unsafe { capture(pcm, buffer.cast::<u8>(), size) };
    if got > 0 {
        // SAFETY: this module's statics.
        unsafe {
            G_AUDIO_IN_CALLS = G_AUDIO_IN_CALLS.wrapping_add(1);
        }
        // SAFETY: the buffer holds the frames just read.
        unsafe {
            if looper && !buffer.is_null() && G_IN_CH != 0 && G_IN_S32 != 0 {
                crate::looper_ffi::looper_input_s32_interleaved(
                    buffer,
                    G_IN_CH,
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
                G_IN_DELAY = u32::try_from(avail).unwrap_or(0);
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
            G_IN_CH = channels;
            G_IN_S32 = s32;
        }
        if frames != 0 && crate::glue::buffer_out() != 0 {
            G_BUF_DIRTY = 0;
        }
    }
}

/// The channel count the app asked the device for.
pub static mut G_PCM_CHANNELS: u32 = 4;

/// The app's own `snd_pcm_readn`.
static mut REAL_READN: Option<unsafe extern "C" fn(usize, *mut *mut i32, u64) -> i64> = None;
/// The app's own `snd_pcm_hw_params_set_channels`.
static mut REAL_SET_CHANNELS: Option<unsafe extern "C" fn(usize, u32) -> i32> = None;

/// The interleaved input tap, for callers that read several buffers at once.
///
/// # Safety
///
/// `buffers` must hold `size` bytes per buffer, as the app's own reader requires.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_readn(pcm: usize, buffers: *mut *mut i32, size: u64) -> i64 {
    // SAFETY: the static is this module's.
    let mut real = unsafe { *core::ptr::addr_of!(REAL_READN) };
    if real.is_none() {
        real = look_up(c"snd_pcm_readn");
        // SAFETY: written once.
        unsafe { REAL_READN = real };
    }
    let Some(capture) = real else {
        return -1;
    };
    // SAFETY: the app's own reader takes these.
    let got = unsafe { capture(pcm, buffers, size) };
    if got <= 0 {
        return got;
    }
    // SAFETY: this module's statics.
    unsafe {
        G_AUDIO_IN_CALLS = G_AUDIO_IN_CALLS.wrapping_add(1);
    }
    let looper = crate::looper_ffi::looper_is_open() != 0;
    // SAFETY: the buffers hold the frames just read.
    unsafe {
        if looper && !buffers.is_null() && !(*buffers).is_null() && G_IN_S32 != 0 {
            crate::looper_ffi::looper_input_s32(
                buffers.cast::<*const i32>(),
                1,
                u32::try_from(got).unwrap_or(0),
            );
        }
    }
    got
}

/// Remember the channel count the app asked for, and pass the request on.
///
/// # Safety
///
/// `params` is the app's own parameters object.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn snd_pcm_hw_params_set_channels(params: usize, channels: u32) -> i32 {
    // SAFETY: the static is this module's.
    let mut real = unsafe { *core::ptr::addr_of!(REAL_SET_CHANNELS) };
    if real.is_none() {
        real = look_up(c"snd_pcm_hw_params_set_channels");
        // SAFETY: written once.
        unsafe { REAL_SET_CHANNELS = real };
    }
    if channels != 0 {
        // SAFETY: this module's static.
        unsafe { G_PCM_CHANNELS = channels };
    }
    match real {
        // SAFETY: the app's own setter takes these.
        Some(set) => unsafe { set(params, channels) },
        None => -1,
    }
}

/// Whether the repaint pump has been started.
static PUMP_STARTED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Start the repaint pump once.
pub extern "C" fn minimod_pump_start() {
    use core::sync::atomic::Ordering;
    if PUMP_STARTED.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("minimod-pump".to_string())
        .spawn(|| {
            // SAFETY: the thread entry owns nothing and never returns.
            let _ = unsafe { minimod_pump_thread(0) };
        });
}

/// The preview's mix: it adds to interleaved S32 output, so its own scratch
/// planes are two channels of the largest block the device writes.
const PREVIEW_BLOCK: usize = 4096;

/// What the output probe found: format, channels and rate of the app's stream.
static PROBE: std::sync::OnceLock<(i32, u32, u32)> = std::sync::OnceLock::new();

/// Whether the mixer is inside the preview already (it is not re-entrant).
static MIXER_BUSY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Whether the previous block carried the preview, so the voices are cleared
/// once when it stops.
static WAS_ACTIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Read the stream's format, channels and rate once.
fn probe(pcm: usize) -> (i32, u32, u32) {
    *PROBE.get_or_init(|| {
        let mut channels = 0_u32;
        let mut s32 = 0_i32;
        // SAFETY: `pcm` is the app's live handle; the out pointers are ours.
        unsafe {
            minimod_stream_geometry(
                pcm,
                core::ptr::from_mut(&mut channels),
                core::ptr::from_mut(&mut s32),
            );
        }
        (s32, channels, stream_rate(pcm))
    })
}

/// The stream's rate, or the device's nominal 44.1 kHz when it cannot be read.
fn stream_rate(pcm: usize) -> u32 {
    type Malloc = unsafe extern "C" fn(*mut *mut libc::c_void) -> i32;
    type Current = unsafe extern "C" fn(*mut libc::c_void, *mut libc::c_void) -> i32;
    type Rate = unsafe extern "C" fn(*mut libc::c_void, *mut u32, *mut i32) -> i32;
    type Free = unsafe extern "C" fn(*mut libc::c_void);
    let (Some(malloc), Some(current), Some(rate), Some(free)) = (
        look_up::<Malloc>(c"snd_pcm_hw_params_malloc"),
        look_up::<Current>(c"snd_pcm_hw_params_current"),
        look_up::<Rate>(c"snd_pcm_hw_params_get_rate"),
        look_up::<Free>(c"snd_pcm_hw_params_free"),
    ) else {
        return 44_100;
    };
    let mut params: *mut libc::c_void = core::ptr::null_mut();
    // SAFETY: the entry point allocates the parameters.
    if unsafe { malloc(core::ptr::from_mut(&mut params)) } < 0 || params.is_null() {
        return 44_100;
    }
    let mut hertz = 0_u32;
    let mut direction = 0_i32;
    // SAFETY: the handle is live and `params` is ours.
    let known = unsafe {
        current(pcm as *mut libc::c_void, params) >= 0
            && rate(
                params,
                core::ptr::from_mut(&mut hertz),
                core::ptr::from_mut(&mut direction),
            ) >= 0
    };
    // SAFETY: the parameters came from the malloc above.
    unsafe { free(params) };
    if known && (8_000..=192_000).contains(&hertz) {
        hertz
    } else {
        44_100
    }
}

/// Add the bass preview to the block the app is about to write.
///
/// The app's real output is `snd_pcm_writei` with interleaved S32; the planar
/// float writer the mod used to hook is not called during normal playback, so
/// mixing there was inaudible.  Channels beyond the first two are untouched.
///
/// # Safety
///
/// `buffer` must hold `frames` interleaved frames of the stream behind `pcm`.
unsafe fn mix_preview(pcm: usize, buffer: *mut i32, frames: u64) {
    use core::sync::atomic::Ordering;
    // SAFETY: a plain word, written by the UI thread.
    let active = unsafe { G_BASS_PREVIEW_ACTIVE } != 0;
    let was = WAS_ACTIVE.swap(active, Ordering::AcqRel);
    let (s32, channels, rate) = probe(pcm);
    let frames = usize::try_from(frames).unwrap_or(0);
    if !active {
        if was {
            // SAFETY: an empty channel list mixes nothing and clears the voices.
            unsafe { minimod_dsp::bass::preview_mix(&mut [], 0, rate) };
        }
        return;
    }
    if buffer.is_null()
        || frames == 0
        || frames > PREVIEW_BLOCK
        || s32 == 0
        || !(1..=8).contains(&channels)
        || MIXER_BUSY.swap(true, Ordering::AcqRel)
    {
        return;
    }
    let count = channels.min(2) as usize;
    let mut planes = vec![0.0_f32; count * frames];
    let (left, right) = planes.split_at_mut(frames);
    let mut pointers: [*mut f32; 2] = [left.as_mut_ptr(), right.as_mut_ptr()];
    // SAFETY: each pointer addresses `frames` writable samples.
    let peak = unsafe { minimod_dsp::bass::preview_mix(&mut pointers[..count], frames, rate) };
    if peak > 0.0 {
        let stride = channels as usize;
        for frame in 0..frames {
            for channel in 0..count {
                let value = if channel == 0 {
                    left[frame]
                } else {
                    right[frame]
                };
                let added = (f64::from(value.clamp(-1.0, 1.0)) * 2_147_483_647.0) as i64;
                // SAFETY: the block holds `frames * stride` samples.
                let slot = unsafe { buffer.add(frame * stride + channel) };
                // SAFETY: see above.
                let sum = i64::from(unsafe { *slot }) + added;
                // SAFETY: see above.
                unsafe { *slot = sum.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32 };
            }
        }
        // SAFETY: counters are this module's statics, written by the audio thread.
        unsafe {
            core::ptr::addr_of_mut!(G_BASS_PREVIEW_BLOCKS)
                .write_volatile(G_BASS_PREVIEW_BLOCKS + 1);
            if peak > f32::from_bits(G_BASS_PREVIEW_PEAK_BITS) {
                core::ptr::addr_of_mut!(G_BASS_PREVIEW_PEAK_BITS).write_volatile(peak.to_bits());
            }
        }
    }
    MIXER_BUSY.store(false, Ordering::Release);
}
