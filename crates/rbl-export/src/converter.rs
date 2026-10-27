//! Conversion to a format the caller encodes: the export decides which tracks need it and records what it
//! made, so a later sync with the same converter reuses the stick's copy instead of encoding again.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Encodes audio for the stick on the caller's behalf.
pub trait AudioConverter: Sync {
    /// Names what the output is, encoder version included. Recorded per converted track; a different
    /// profile converts the track again.
    fn profile(&self) -> &str;
    /// The output's file extension, without the dot.
    fn extension(&self) -> &str;
    /// Whether `source` needs converting. A track that doesn't is copied as it is.
    fn wants(&self, source: &Path) -> std::io::Result<bool>;
    /// Encode `source` to `destination`.
    fn convert(&self, source: &Path, destination: &Path) -> std::io::Result<ConvertedAudio>;
    /// One analysis file (`extension` is `DAT`, `EXT` or `2EX`) moved to the converted audio's timeline.
    fn shift_analysis(&self, extension: &str, bytes: &[u8], offset_ms: i64) -> std::io::Result<Vec<u8>>;
    /// Roughly how many bytes a track of `duration_sec` converts to.
    fn estimate_bytes(&self, duration_sec: u64) -> u64;
}

/// What a conversion produced, as the device databases describe it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConvertedAudio {
    /// How far the audio's timeline moved: cues, the beat grid and the detail waveforms move by this.
    pub offset_ms: i64,
    pub bitrate: u32,
    pub sample_rate: u32,
    pub bit_depth: u16,
}

/// What one track is converted with.
#[derive(Clone, Copy)]
pub(crate) enum Target<'a> {
    Builtin(rbl_audio::compatibility::Format),
    External(&'a dyn AudioConverter),
}

impl Target<'_> {
    pub(crate) fn profile(&self) -> &str {
        match self {
            Self::Builtin(format) => format.profile(),
            Self::External(converter) => converter.profile(),
        }
    }

    pub(crate) fn extension(&self) -> &str {
        match self {
            Self::Builtin(format) => format.extension(),
            Self::External(converter) => converter.extension(),
        }
    }

    pub(crate) fn builtin(self) -> Option<rbl_audio::compatibility::Format> {
        match self {
            Self::Builtin(format) => Some(format),
            Self::External(_) => None,
        }
    }
}

/// `cues` on a timeline moved by `offset_ms`, kept within `0..=length_ms` when the length is known.
pub(crate) fn shift_cues(cues: &mut [rbl_anlz::cues::ExportCue], offset_ms: i64, length_ms: u64) {
    let limit = if length_ms == 0 { i64::from(u32::MAX) } else { i64::try_from(length_ms).unwrap_or(i64::MAX) };
    let moved = |ms: u32| u32::try_from((i64::from(ms) + offset_ms).clamp(0, limit)).unwrap_or(u32::MAX);
    for cue in cues {
        cue.time_ms = moved(cue.time_ms);
        cue.loop_time_ms = cue.loop_time_ms.map(|end| moved(end).max(cue.time_ms));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_move_and_stay_on_the_track() {
        let cue = |time_ms, loop_time_ms| rbl_anlz::cues::ExportCue { time_ms, loop_time_ms, ..Default::default() };
        let mut cues = vec![cue(10, None), cue(1000, Some(2000)), cue(4990, Some(4995))];
        shift_cues(&mut cues, 23, 5000);
        assert_eq!(cues.iter().map(|c| (c.time_ms, c.loop_time_ms)).collect::<Vec<_>>(), [(33, None), (1023, Some(2023)), (5000, Some(5000))]);
        shift_cues(&mut cues, -40, 0);
        assert_eq!(cues[0].time_ms, 0);
    }
}
