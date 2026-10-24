//! Where rekordbox's timeline starts in an audio file. Every answer comes from the file itself.

use std::fs::File;
use std::path::Path;

/// Source-rate frames rekordbox's timeline runs ahead of a plain decode of the file: rekordbox's time
/// `t` is the decode's `t - lead`. Zero when the file is not an MP3 or cannot be read.
#[must_use]
pub fn rekordbox_lead_frames(path: &Path) -> i64 {
    File::open(path).map_or(0, |mut file| rbl_core::mpeg::rekordbox_lead_frames(&mut file))
}

/// Seconds from the start of the audio as it was encoded to rekordbox's time 0. `None` when the file
/// does not say, so nothing is assumed.
#[must_use]
pub fn content_start_seconds(path: &Path) -> Option<f64> {
    rbl_core::mpeg::content_start_seconds(&mut File::open(path).ok()?)
}

/// Milliseconds that times on rekordbox's timeline move when `output` replaces `source` as the same
/// audio. `None` unless both files say what their delay is.
#[must_use]
pub fn conversion_shift_ms(source: &Path, output: &Path) -> Option<i64> {
    rbl_core::mpeg::conversion_shift_ms(&mut File::open(source).ok()?, &mut File::open(output).ok()?)
}
