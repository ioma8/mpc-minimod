//! What the feature asks the SDK render job to do.
//!
//! The action codes are the mod's vocabulary; the SDK carries them as an
//! opaque `kind`.  This module maps each code to the backend that fills the
//! job's outputs.

use mpc_sample_sdk::platform::selection::Selection;
use mpc_sample_sdk::render::RenderOutput;

use crate::backend::{bass, hpss};

/// The work MINIMOD schedules. A selected-sample action retains its validated
/// SDK selection; generated synth audio needs only the Sample page identity.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Job {
    pub(crate) source: Option<Selection>,
    pub(crate) screen: usize,
    pub(crate) program: Option<usize>,
    pub(crate) destination: Option<i32>,
    pub(crate) frames: u32,
    pub(crate) channels: u16,
    pub(crate) rate: u32,
    pub(crate) kind: i32,
    pub(crate) input: Vec<f32>,
    pub(crate) outputs: Vec<RenderOutput>,
}

impl Job {
    pub(crate) fn selected(source: Selection, kind: i32) -> Self {
        Self {
            screen: source.screen(),
            program: None,
            destination: None,
            frames: source.frames(),
            channels: source.channels(),
            rate: source.rate(),
            source: Some(source),
            kind,
            input: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub(crate) fn generated(screen: usize, kind: i32) -> Self {
        Self {
            source: None,
            screen,
            program: None,
            destination: None,
            frames: 0,
            channels: 2,
            rate: 44_100,
            kind,
            input: Vec::new(),
            outputs: Vec::new(),
        }
    }

    pub(crate) fn push_output(&mut self, output: RenderOutput) {
        self.outputs.push(output);
    }
}

/// One render the SAMPLE OPTIONS menu can start.
///
/// MINIMOD carries the code as an opaque `i32` in its `Job::kind`; this is
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

    /// The code a job carries.
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
pub(crate) fn render(job: &mut Job, knobs: &[f32], kind: bass::Kind, note: i32) -> Result<(), i32> {
    let action = Action::from_code(job.kind).ok_or(job.kind)?;
    let channels = usize::from(job.channels);
    let rate = job.rate;
    match action {
        Action::Split => {
            let Some((harmonic, percussive)) = hpss::split(&job.input, channels, rate) else {
                return Err(job.kind);
            };
            let frames = usize::try_from(job.frames).unwrap_or(0);
            let channels = job.channels;
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
                u32::from(job.channels),
                rate,
                parameters,
            ) else {
                return Err(job.kind);
            };
            job.push_output(RenderOutput::new(audio, u32::from(job.channels)).ok_or(job.kind)?);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, Job, render};
    use crate::backend::bass;

    #[test]
    fn an_unknown_action_reaches_the_effect_path_and_fails_without_an_image() {
        let mut job = Job::generated(1, 9);
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
