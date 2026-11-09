//! The render job: one action's input, output and outcome.
//!
//! The glue creates the job (it owns the sample read and the source backup),
//! starts the worker and frees it.  The layout is defined here, once, and the
//! glue uses the same field names through its header.

/// The HPSS split action.
pub const ACTION_SPLIT: i32 = 0;
/// The bass synth action.
pub const ACTION_BASS: i32 = 4;

/// One render job.
#[repr(C)]
#[derive(Debug)]
pub struct Job {
    /// The page the action was started from.
    pub screen: usize,
    /// The page state at that moment.
    pub state: usize,
    /// The selected sample.
    pub sample: usize,
    /// Its length in frames.
    pub frames: u32,
    /// Its channel count.
    pub channels: u32,
    /// Its sample rate.
    pub rate: u32,
    /// The interleaved input.
    pub input: *mut f32,
    /// The harmonic part of a split.
    pub harmonic: *mut f32,
    /// The percussive part of a split.
    pub percussive: *mut f32,
    /// A rendered effect or synth take.
    pub rendered: *mut f32,
    /// Its length in frames.
    pub rendered_frames: usize,
    /// Its channel count.
    pub rendered_channels: u32,
    /// Which action this is.
    pub action: i32,
    /// The worker's result: 0 for success, anything else for failure.
    pub rc: i32,
}

/// Run the job's action: render an effect or a synth take, or split the input.
///
/// The three implementations are already Rust; this only picks one and records
/// the result.  The synth's knobs are passed in rather than read from a second
/// copy of the state.
///
/// # Safety
///
/// `job` must be a live job with its input filled in, run by one thread.
pub unsafe extern "C" fn minimod_job_run(job: *mut Job, knobs: *const f32, app_base: usize) -> i32 {
    if job.is_null() {
        return -1;
    }
    // SAFETY: the caller promises the job is live and only this thread runs it.
    let job = unsafe { &mut *job };
    let rc = match job.action {
        ACTION_BASS => {
            let (harmonics, drive, cutoff) = if knobs.is_null() {
                (0.5, 0.5, 0.5)
            } else {
                // SAFETY: the caller promises three readable knob values.
                let values = unsafe { core::slice::from_raw_parts(knobs, 3) };
                (values[0], values[1], values[2])
            };
            // SAFETY: the outputs are the job's own fields.
            unsafe {
                crate::bass_ffi::bass_sample_render(
                    job.rate,
                    harmonics,
                    drive,
                    cutoff,
                    core::ptr::addr_of_mut!(job.rendered),
                    core::ptr::addr_of_mut!(job.rendered_frames),
                )
            }
        }
        ACTION_SPLIT => {
            // SAFETY: the job's input is live for its geometry, and the outputs
            // are its own fields.
            unsafe {
                crate::analysis_ffi::hpss_split(
                    job.input,
                    job.frames as usize,
                    job.channels,
                    job.rate,
                    core::ptr::addr_of_mut!(job.harmonic),
                    core::ptr::addr_of_mut!(job.percussive),
                )
            }
        }
        action => {
            // SAFETY: as above.
            unsafe {
                crate::effect_ffi::h3m_effect_render(
                    app_base,
                    action - 1,
                    job.input,
                    job.frames as usize,
                    job.channels,
                    job.rate,
                    core::ptr::addr_of_mut!(job.rendered),
                    core::ptr::addr_of_mut!(job.rendered_frames),
                )
            }
        }
    };
    if job.action == ACTION_BASS {
        job.rendered_channels = 2;
    }
    job.rc = rc;
    rc
}

/// Free the buffers a job owns and the job itself.
///
/// # Safety
///
/// `job` must have come from the glue's allocation and not be used afterwards.
pub unsafe extern "C" fn minimod_job_free(job: *mut Job) {
    if job.is_null() {
        return;
    }
    // SAFETY: the caller promises the job is ours to free.
    unsafe {
        let fields = &mut *job;
        libc::free(fields.input.cast());
        libc::free(fields.harmonic.cast());
        libc::free(fields.percussive.cast());
        libc::free(fields.rendered.cast());
        libc::free(job.cast());
    }
}
