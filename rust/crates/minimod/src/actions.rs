//! What the feature asks the SDK render job to do.
//!
//! The action codes are the mod's vocabulary; the SDK carries them as an
//! opaque `kind`.  This module maps each code to the backend that fills the
//! job's outputs.

use mpc_sample_sdk::render::{RenderJob, RenderOutput};

use crate::backend::{bass, hpss};

/// One render the SAMPLE OPTIONS menu can start.
///
/// The SDK carries the code as an opaque `i32` in [`RenderJob::kind`]; this is
/// the only place that mapping exists, so an unknown code is an error instead of
/// silently becoming an effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(i32)]
pub enum Action {
    /// Harmonic/percussive split.
    Split = 0,
    /// Reverb.
    Reverb = 1,
    /// Vinyl.
    Vinyl = 2,
    /// Lo-fi decimator.
    LoFi = 3,
    /// Bass synth.
    Bass = 4,
}

impl Action {
    /// Every action, in menu order.
    pub const ALL: [Self; 5] = [
        Self::Split,
        Self::Reverb,
        Self::Vinyl,
        Self::LoFi,
        Self::Bass,
    ];

    /// The code a [`RenderJob`] carries.
    #[must_use]
    pub const fn code(self) -> i32 {
        self as i32
    }

    /// The action for a job code.
    #[must_use]
    pub const fn from_code(code: i32) -> Option<Self> {
        match code {
            0 => Some(Self::Split),
            1 => Some(Self::Reverb),
            2 => Some(Self::Vinyl),
            3 => Some(Self::LoFi),
            4 => Some(Self::Bass),
            _ => None,
        }
    }

    /// Whether this action renders one take rather than the two-way split.
    #[must_use]
    pub const fn is_effect(self) -> bool {
        !matches!(self, Self::Split)
    }
}

/// Render `job` according to its action code.
///
/// The error code is the action code, so the feature can name the failed action
/// after the SDK has dropped the job.
///
/// # Errors
///
/// Returns the action code when the backend cannot produce the expected output.
pub fn render(job: &mut RenderJob, knobs: &[f32], kind: bass::Kind, note: i32) -> Result<(), i32> {
    let action = Action::from_code(job.kind).ok_or(job.kind)?;
    let channels = usize::from(job.source.channels());
    let rate = job.source.rate();
    match action {
        Action::Split => {
            let Some((harmonic, percussive)) = hpss::split(&job.input, channels, rate) else {
                return Err(job.kind);
            };
            let frames = usize::try_from(job.source.frames()).unwrap_or(0);
            let channels = job.source.channels();
            job.push_output(
                RenderOutput::with_frames(harmonic, frames, u32::from(channels)).ok_or(job.kind)?,
            );
            job.push_output(
                RenderOutput::with_frames(percussive, frames, u32::from(channels))
                    .ok_or(job.kind)?,
            );
            Ok(())
        }
        Action::Bass => {
            let params = match knobs {
                [a, b, c, ..] => [*a, *b, *c],
                _ => [0.5, 0.25, 0.5],
            };
            let Some((audio, frames)) = bass::render(kind, rate, params, note) else {
                return Err(job.kind);
            };
            job.push_output(RenderOutput::with_frames(audio, frames, 2).ok_or(job.kind)?);
            Ok(())
        }
        action => {
            let parameters: &[(usize, f32)] = match action {
                Action::Reverb => &[],
                Action::Vinyl => &[(0, 0.18), (5, 0.12)],
                Action::LoFi => &[(0, 0.58), (4, 0.70)],
                _ => return Err(job.kind),
            };
            let Some(audio) = mpc_sample_sdk::platform::effect::render_code(
                action.code() - 1,
                &job.input,
                job.source.channels().into(),
                rate,
                parameters,
            ) else {
                return Err(job.kind);
            };
            job.push_output(
                RenderOutput::new(audio, u32::from(job.source.channels())).ok_or(job.kind)?,
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, render};
    use crate::backend::bass;
    use mpc_sample_sdk::platform::selection::Selection;
    use mpc_sample_sdk::render::RenderJob;

    fn source() -> Selection {
        // SAFETY: the test only carries geometry; `render` never reaches the
        // firmware because the effect path is uninstalled on the host.
        unsafe { Selection::from_parts(1, 2, 3, 4, 2, 44_100) }
    }

    #[test]
    fn an_unknown_action_reaches_the_effect_path_and_fails_without_an_image() {
        let mut job = RenderJob::new(source(), 9, vec![0.0; 8]);
        assert_eq!(render(&mut job, &[], bass::Kind::Bass, 0), Err(9));
        assert!(job.outputs.is_empty());
    }

    #[test]
    fn the_action_codes_round_trip() {
        for (index, action) in Action::ALL.iter().enumerate() {
            assert_eq!(
                i32::try_from(index).unwrap_or(-1),
                action.code(),
                "{action:?}"
            );
            assert_eq!(Action::from_code(action.code()), Some(*action));
        }
        assert_eq!(Action::from_code(-1), None);
        assert_eq!(Action::from_code(9), None);
    }
}
