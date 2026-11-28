//! The song structure (`PSSI`): the phrases rekordbox draws above the waveform, laid out as
//! Deep Symmetry's ANLZ notes describe.

use std::fmt;

use crate::decode::{be16, be32, Tags};
use crate::PHRASE_TAG;

const ENTRY_BYTES_AT: usize = 12;
const ENTRY_COUNT_AT: usize = 16;
const BODY_AT: usize = 18;

const END_BEAT_IN_BODY: usize = 8;
const ENTRIES_IN_BODY: usize = 14;
const SMALLEST_ENTRY: usize = 24;

const FIRST_BEAT_IN_ENTRY: usize = 2;
const KIND_IN_ENTRY: usize = 4;
const INTRO_CHORUS_OUTRO_VARIANT_IN_ENTRY: usize = 0x07;
const IS_UP_3_IN_ENTRY: usize = 0x09;
const IS_UP_2_IN_ENTRY: usize = 0x13;
const HAS_FILL_IN_IN_ENTRY: usize = 0x15;
const FILL_IN_BEAT_IN_ENTRY: usize = 0x16;

/// rekordbox 6 XORs the body with this pad, every byte raised by the phrase count. rekordbox 7
/// leaves the files in its own library plain, and nothing in the file says which it is.
const MASK: [u8; 19] =
    [0xCB, 0xE1, 0xEE, 0xFA, 0xE5, 0xEE, 0xAD, 0xEE, 0xE9, 0xD2, 0xE9, 0xEB, 0xE1, 0xE9, 0xF3, 0xE8, 0xE9, 0xF4, 0xE1];

/// The character rekordbox gives the whole track, which decides what each phrase kind means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mood {
    High,
    Mid,
    Low,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhraseKind {
    Intro,
    Verse,
    Up,
    Down,
    Bridge,
    Chorus,
    Outro,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Phrase {
    /// 1-based, counting the beats of the `PQTZ` grid.
    pub first_beat: u16,
    /// `None` for a kind rekordbox has no name for.
    pub kind: Option<PhraseKind>,
    pub variant: Option<u8>,
    pub fill_in_beat: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SongStructure {
    pub mood: Mood,
    /// Where the last phrase ends; the audio may run on past it.
    pub end_beat: u16,
    pub phrases: Vec<Phrase>,
}

impl fmt::Display for Phrase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self.kind {
            None => "?",
            Some(PhraseKind::Intro) => "INTRO",
            Some(PhraseKind::Verse) => "VERSE",
            Some(PhraseKind::Up) => "UP",
            Some(PhraseKind::Down) => "DOWN",
            Some(PhraseKind::Bridge) => "BRIDGE",
            Some(PhraseKind::Chorus) => "CHORUS",
            Some(PhraseKind::Outro) => "OUTRO",
        };
        match self.variant {
            Some(variant) => write!(f, "{name} {variant}"),
            None => f.write_str(name),
        }
    }
}

/// The song structure, or none when the file has no readable `PSSI`.
#[must_use]
pub fn song_structure(ext: &Tags) -> Option<SongStructure> {
    let tag = ext.first(PHRASE_TAG)?;
    let entry_bytes = be32(tag, ENTRY_BYTES_AT)? as usize;
    let phrase_count = be16(tag, ENTRY_COUNT_AT)?;
    if entry_bytes < SMALLEST_ENTRY {
        return None;
    }
    let mut body = tag.get(BODY_AT..)?.to_vec();
    let mood = match read_mood(&body) {
        Some(mood) => mood,
        None => {
            unmask(&mut body, phrase_count);
            read_mood(&body)?
        }
    };
    let phrases = body
        .get(ENTRIES_IN_BODY..)?
        .chunks_exact(entry_bytes)
        .take(usize::from(phrase_count))
        .filter_map(|entry| read_phrase(mood, entry))
        .collect();
    Some(SongStructure { mood, end_beat: be16(&body, END_BEAT_IN_BODY)?, phrases })
}

fn read_mood(body: &[u8]) -> Option<Mood> {
    match be16(body, 0)? {
        1 => Some(Mood::High),
        2 => Some(Mood::Mid),
        3 => Some(Mood::Low),
        _ => None,
    }
}

fn unmask(body: &mut [u8], phrase_count: u16) {
    let raise = phrase_count.to_be_bytes()[1];
    for (byte, pad) in body.iter_mut().zip(MASK.iter().cycle()) {
        *byte ^= pad.wrapping_add(raise);
    }
}

fn read_phrase(mood: Mood, entry: &[u8]) -> Option<Phrase> {
    let (kind, variant) = kind_and_variant(mood, be16(entry, KIND_IN_ENTRY)?, entry);
    let has_fill_in = entry.get(HAS_FILL_IN_IN_ENTRY).is_some_and(|&flag| flag != 0);
    Some(Phrase {
        first_beat: be16(entry, FIRST_BEAT_IN_ENTRY)?,
        kind,
        variant,
        fill_in_beat: if has_fill_in { be16(entry, FILL_IN_BEAT_IN_ENTRY) } else { None },
    })
}

fn kind_and_variant(mood: Mood, kind: u16, entry: &[u8]) -> (Option<PhraseKind>, Option<u8>) {
    let flag = |at: usize| entry.get(at).copied().unwrap_or(0);
    let first_or_second = if flag(INTRO_CHORUS_OUTRO_VARIANT_IN_ENTRY) == 1 { 1 } else { 2 };
    let up_variant = if flag(IS_UP_3_IN_ENTRY) != 0 {
        3
    } else if flag(IS_UP_2_IN_ENTRY) != 0 {
        2
    } else {
        1
    };
    match (mood, kind) {
        (Mood::High, 1) => (Some(PhraseKind::Intro), Some(first_or_second)),
        (Mood::High, 2) => (Some(PhraseKind::Up), Some(up_variant)),
        (Mood::High, 3) => (Some(PhraseKind::Down), None),
        (Mood::High, 5) => (Some(PhraseKind::Chorus), Some(first_or_second)),
        (Mood::High, 6) => (Some(PhraseKind::Outro), Some(first_or_second)),
        (Mood::Mid | Mood::Low, 1) => (Some(PhraseKind::Intro), None),
        (Mood::Mid, 2..=7) => (Some(PhraseKind::Verse), u8::try_from(kind - 1).ok()),
        (Mood::Low, 2..=4) => (Some(PhraseKind::Verse), Some(1)),
        (Mood::Low, 5..=7) => (Some(PhraseKind::Verse), Some(2)),
        (Mood::Mid | Mood::Low, 8) => (Some(PhraseKind::Bridge), None),
        (Mood::Mid | Mood::Low, 9) => (Some(PhraseKind::Chorus), None),
        (Mood::Mid | Mood::Low, 10) => (Some(PhraseKind::Outro), None),
        _ => (None, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Entry {
        first_beat: u16,
        kind: u16,
        intro_chorus_outro_variant: u8,
        is_up_2: u8,
        is_up_3: u8,
        fill_in_beat: Option<u16>,
    }

    fn entry(first_beat: u16, kind: u16) -> Entry {
        Entry { first_beat, kind, intro_chorus_outro_variant: 0, is_up_2: 0, is_up_3: 0, fill_in_beat: None }
    }

    fn pssi(mood: u16, end_beat: u16, entries: &[Entry], masked: bool) -> Vec<u8> {
        let count = u16::try_from(entries.len()).unwrap();
        let mut body = mood.to_be_bytes().to_vec();
        body.extend([0; 6]);
        body.extend(end_beat.to_be_bytes());
        body.extend([0; 4]);
        for (index, e) in entries.iter().enumerate() {
            let mut bytes = [0u8; SMALLEST_ENTRY];
            bytes[0..2].copy_from_slice(&u16::try_from(index + 1).unwrap().to_be_bytes());
            bytes[2..4].copy_from_slice(&e.first_beat.to_be_bytes());
            bytes[4..6].copy_from_slice(&e.kind.to_be_bytes());
            bytes[INTRO_CHORUS_OUTRO_VARIANT_IN_ENTRY] = e.intro_chorus_outro_variant;
            bytes[IS_UP_2_IN_ENTRY] = e.is_up_2;
            bytes[IS_UP_3_IN_ENTRY] = e.is_up_3;
            if let Some(beat) = e.fill_in_beat {
                bytes[HAS_FILL_IN_IN_ENTRY] = 1;
                bytes[FILL_IN_BEAT_IN_ENTRY..].copy_from_slice(&beat.to_be_bytes());
            }
            body.extend(bytes);
        }
        if masked {
            unmask(&mut body, count);
        }
        let mut tag = b"PSSI".to_vec();
        tag.extend(32u32.to_be_bytes());
        tag.extend(u32::try_from(BODY_AT + body.len()).unwrap().to_be_bytes());
        tag.extend(u32::try_from(SMALLEST_ENTRY).unwrap().to_be_bytes());
        tag.extend(count.to_be_bytes());
        tag.extend(body);
        let mut file = b"PMAI".to_vec();
        file.extend(28u32.to_be_bytes());
        file.extend(u32::try_from(28 + tag.len()).unwrap().to_be_bytes());
        file.extend([0u8; 16]);
        file.extend(tag);
        file
    }

    fn festival_anthem() -> Vec<Entry> {
        vec![
            Entry { intro_chorus_outro_variant: 1, ..entry(1, 1) },
            entry(65, 2),
            Entry { is_up_2: 1, ..entry(129, 2) },
            Entry { intro_chorus_outro_variant: 1, fill_in_beat: Some(253), ..entry(193, 5) },
            entry(257, 3),
            Entry { is_up_3: 1, ..entry(321, 2) },
            entry(385, 5),
            entry(449, 6),
        ]
    }

    fn labels(structure: &SongStructure) -> Vec<String> {
        structure.phrases.iter().map(ToString::to_string).collect()
    }

    #[test]
    fn reads_a_high_mood_track_with_its_variants() {
        let ext = Tags::parse(pssi(1, 513, &festival_anthem(), false)).unwrap();
        let structure = song_structure(&ext).unwrap();
        assert_eq!(structure.mood, Mood::High);
        assert_eq!(structure.end_beat, 513);
        assert_eq!(labels(&structure), ["INTRO 1", "UP 1", "UP 2", "CHORUS 1", "DOWN", "UP 3", "CHORUS 2", "OUTRO 2"]);
        assert_eq!(structure.phrases[3].fill_in_beat, Some(253));
        assert_eq!(structure.phrases[4].first_beat, 257);
    }

    #[test]
    fn masked_and_plain_files_read_the_same() {
        let plain = song_structure(&Tags::parse(pssi(1, 513, &festival_anthem(), false)).unwrap());
        let masked = song_structure(&Tags::parse(pssi(1, 513, &festival_anthem(), true)).unwrap());
        assert_eq!(plain, masked);
    }

    #[test]
    fn mid_and_low_moods_name_verses_differently() {
        let entries = || vec![entry(1, 1), entry(33, 4), entry(65, 6), entry(97, 9), entry(129, 10)];
        let mid = song_structure(&Tags::parse(pssi(2, 161, &entries(), false)).unwrap()).unwrap();
        let low = song_structure(&Tags::parse(pssi(3, 161, &entries(), false)).unwrap()).unwrap();
        assert_eq!(labels(&mid), ["INTRO", "VERSE 3", "VERSE 5", "CHORUS", "OUTRO"]);
        assert_eq!(labels(&low), ["INTRO", "VERSE 1", "VERSE 2", "CHORUS", "OUTRO"]);
    }

    #[test]
    fn agrees_with_the_reference_reader() {
        for masked in [false, true] {
            let data = pssi(1, 513, &festival_anthem(), masked);
            let reference = rbl_anlz::parse(&data).unwrap().song_structure().unwrap();
            let ours = song_structure(&Tags::parse(data).unwrap()).unwrap();
            let theirs: Vec<(u16, &str, u16)> =
                reference.phrases.iter().map(|p| (p.beat, p.label, p.fill_in_beat)).collect();
            let ours: Vec<(u16, String, u16)> =
                ours.phrases.iter().map(|p| (p.first_beat, p.to_string(), p.fill_in_beat.unwrap_or(0))).collect();
            assert_eq!(ours.len(), theirs.len());
            for ((beat, label, fill), (ref_beat, ref_label, ref_fill)) in ours.iter().zip(&theirs) {
                assert_eq!((beat, label.as_str(), fill), (ref_beat, *ref_label, ref_fill));
            }
        }
    }

    #[test]
    fn files_without_a_readable_structure_have_none() {
        let mut garbled = pssi(1, 513, &festival_anthem(), false);
        let body_start = 28 + BODY_AT;
        garbled[body_start..body_start + 2].copy_from_slice(&7u16.to_be_bytes());
        assert_eq!(song_structure(&Tags::parse(garbled).unwrap()), None);
    }
}
