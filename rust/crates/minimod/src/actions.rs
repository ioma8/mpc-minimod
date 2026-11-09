//! What the feature asks the SDK render job to do.
//!
//! The action codes are the mod's vocabulary; the SDK carries them as an
//! opaque `kind`.  This module maps each code to the backend that fills the
//! job's outputs.

use mpc_sample_sdk::render::{RenderJob, RenderOutput};

use crate::backend::{bass, hpss};

/// The HPSS split action.
pub const ACTION_SPLIT: i32 = 0;
/// The reverb action.
pub const ACTION_REVERB: i32 = 1;
/// The vinyl action.
pub const ACTION_VINYL: i32 = 2;
/// The lo-fi action.
pub const ACTION_LOFI: i32 = 3;
/// The bass synth action.
pub const ACTION_BASS: i32 = 4;

/// Render `job` according to its action code.
///
/// The error code is the action code, so the feature can name the failed action
/// after the SDK has dropped the job.
///
/// # Errors
///
/// Returns the action code when the backend cannot produce the expected output.
pub fn render(job: &mut RenderJob, knobs: &[f32]) -> Result<(), i32> {
    let channels = usize::from(job.source.channels);
    let rate = job.source.rate;
    match job.kind {
        ACTION_SPLIT => {
            let Some((harmonic, percussive)) = hpss::split(&job.input, channels, rate) else {
                return Err(job.kind);
            };
            let frames = usize::try_from(job.source.frames).unwrap_or(0);
            let channels = job.source.channels;
            job.push_output(
                RenderOutput::with_frames(harmonic, frames, u32::from(channels)).ok_or(job.kind)?,
            );
            job.push_output(
                RenderOutput::with_frames(percussive, frames, u32::from(channels))
                    .ok_or(job.kind)?,
            );
            Ok(())
        }
        ACTION_BASS => {
            let (harmonics, drive, cutoff) = match knobs {
                [harmonics, drive, cutoff, ..] => (*harmonics, *drive, *cutoff),
                _ => (0.5, 0.5, 0.5),
            };
            let Some((audio, frames)) = bass::render(rate, harmonics, drive, cutoff) else {
                return Err(job.kind);
            };
            job.push_output(RenderOutput::with_frames(audio, frames, 2).ok_or(job.kind)?);
            Ok(())
        }
        kind => {
            let parameters: &[(usize, f32)] = match kind {
                ACTION_VINYL => &[(0, 0.18), (5, 0.12)],
                ACTION_LOFI => &[(0, 0.58), (4, 0.70)],
                _ => &[],
            };
            let Some(audio) = mpc_sample_sdk::platform::effect::render_code(
                kind - 1,
                &job.input,
                job.source.channels.into(),
                rate,
                parameters,
            ) else {
                return Err(job.kind);
            };
            job.push_output(
                RenderOutput::new(audio, u32::from(job.source.channels)).ok_or(job.kind)?,
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ACTION_BASS, ACTION_SPLIT, render};
    use mpc_sample_sdk::platform::selection::Selection;
    use mpc_sample_sdk::render::RenderJob;

    fn source() -> Selection {
        Selection {
            screen: 1,
            state: 2,
            sample: 3,
            frames: 4,
            channels: 2,
            rate: 44_100,
        }
    }

    #[test]
    fn an_unknown_action_reaches_the_effect_path_and_fails_without_an_image() {
        let mut job = RenderJob::new(source(), 9, vec![0.0; 8]);
        assert_eq!(render(&mut job, &[]), Err(9));
        assert!(job.outputs.is_empty());
    }

    #[test]
    fn the_action_codes_match_their_names() {
        assert_eq!(ACTION_SPLIT, 0);
        assert_eq!(ACTION_BASS, 4);
    }
}
