//! A caller's converter: which tracks it converts, what the stick records, and that a second sync with the
//! same converter changes nothing.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rbl_export::{export_with_options, AudioConverter, ConvertedAudio, ExportOptions, Manifest, SourcePlaylist, SourceTrack};

const OFFSET_MS: i64 = 23;

/// "Converts" anything that isn't an MP3 by copying it with a suffix, and moves the beat grid by [`OFFSET_MS`].
#[derive(Default)]
struct Fake {
    conversions: AtomicUsize,
    shifted: Mutex<Vec<(String, i64)>>,
    fail: Option<std::io::ErrorKind>,
}

impl AudioConverter for Fake {
    fn profile(&self) -> &str { "fake-mp3-320-v1" }
    fn extension(&self) -> &str { "mp3" }
    fn wants(&self, source: &Path) -> std::io::Result<bool> { Ok(source.extension().is_none_or(|e| e != "mp3")) }
    fn convert(&self, source: &Path, destination: &Path) -> std::io::Result<ConvertedAudio> {
        if let Some(kind) = self.fail { return Err(kind.into()) }
        self.conversions.fetch_add(1, Ordering::SeqCst);
        std::fs::write(destination, [std::fs::read(source)?, b"-converted".to_vec()].concat())?;
        Ok(ConvertedAudio { offset_ms: OFFSET_MS, bitrate: 320, sample_rate: 44_100, bit_depth: 0 })
    }
    fn shift_analysis(&self, extension: &str, bytes: &[u8], offset_ms: i64) -> std::io::Result<Vec<u8>> {
        self.shifted.lock().unwrap().push((extension.to_owned(), offset_ms));
        let anlz = rbl_anlz::parse(bytes).map_err(std::io::Error::other)?;
        let Some(beats) = anlz.beat_grid() else { return Ok(bytes.to_vec()) };
        let moved: Vec<_> = beats.into_iter().map(|b| rbl_anlz::Beat { time_ms: (i64::from(b.time_ms) + offset_ms) as u32, ..b }).collect();
        Ok(rbl_anlz::AnlzBuilder::new().path("/moved").beat_grid(&moved).finish())
    }
    fn estimate_bytes(&self, duration_sec: u64) -> u64 { duration_sec * 40_000 }
}

fn track(dir: &Path, id: u64, name: &str) -> SourceTrack {
    let path = dir.join(name);
    std::fs::write(&path, vec![id as u8; 4096]).unwrap();
    let beats = [rbl_anlz::Beat { beat_number: 1, tempo_x100: 12800, time_ms: 75 }];
    SourceTrack {
        id, source_path: path, title: name.into(), artist: "Artist".into(), duration_sec: 60,
        analysis: vec![("DAT".into(), rbl_anlz::AnlzBuilder::new().path("/x").beat_grid(&beats).finish())],
        ..Default::default()
    }
}

fn entry<'a>(manifest: &'a Manifest, id: u64) -> &'a rbl_export::ManifestTrack {
    manifest.tracks.iter().find(|t| t.library_id == id).unwrap()
}

fn grid(dest: &Path, entry: &rbl_export::ManifestTrack) -> Vec<u32> {
    let dat = dest.join(entry.anlz_dir.trim_start_matches('/')).join("ANLZ0000.DAT");
    rbl_anlz::Anlz::read(&dat).unwrap().beat_grid().unwrap().iter().map(|b| b.time_ms).collect()
}

#[test]
fn converts_what_it_wants_and_a_second_sync_reuses_it() {
    let src = tempfile::tempdir().unwrap();
    let dest = common::stick();
    let tracks = vec![track(src.path(), 1, "First.wav"), track(src.path(), 2, "Second.mp3")];
    let playlists = vec![SourcePlaylist { id: 1, name: "Set".into(), track_indices: vec![0, 1], ..Default::default() }];
    let fake = Fake::default();
    let options = ExportOptions { converter: Some(&fake), ..Default::default() };

    let report = export_with_options(dest.path(), &tracks, &playlists, &[], &options, &mut |_| {}).unwrap();
    assert_eq!((report.tracks, report.converted), (2, 1));
    assert_eq!(fake.conversions.load(Ordering::SeqCst), 1);
    let manifest = Manifest::load(dest.path()).unwrap();
    let converted = entry(&manifest, 1);
    assert!(converted.audio.ends_with("/First.mp3"), "{}", converted.audio);
    assert_eq!(converted.conversion, "fake-mp3-320-v1");
    assert_eq!(converted.conversion_info.unwrap().offset_ms, OFFSET_MS);
    assert_eq!(grid(dest.path(), converted), [75 + OFFSET_MS as u32]);
    let copied = entry(&manifest, 2);
    assert!(copied.audio.ends_with("/Second.mp3") && copied.conversion.is_empty() && copied.conversion_info.is_none());
    assert_eq!(grid(dest.path(), copied), [75]);
    assert_eq!(*fake.shifted.lock().unwrap(), [("DAT".to_owned(), OFFSET_MS)]);

    let pdb_bytes = std::fs::read(dest.path().join("PIONEER/rekordbox/export.pdb")).unwrap();
    let pdb = rbl_pdb::Pdb::parse(&pdb_bytes).unwrap();
    let rows = pdb.track_rows(pdb.table(rbl_pdb::PageType::Tracks).unwrap());
    let row = rows.iter().find(|r| r.file_path == converted.audio).unwrap();
    assert_eq!((row.bitrate, row.sample_rate), (320, 44_100));
    assert!(rbl_export::verify(dest.path()).unwrap().is_ok());

    let review = rbl_export::review::inspect_volume(dest.path(), &tracks, &playlists, &[], &options).unwrap();
    let plan = review.plan.unwrap();
    assert!(plan.up_to_date);
    assert_eq!((plan.copy, plan.convert, plan.keep, plan.remove), (0, 0, 2, 0));

    let again = export_with_options(dest.path(), &tracks, &playlists, &[], &options, &mut |_| {}).unwrap();
    assert_eq!((again.bytes_copied, again.analysis_files, again.converted), (0, 0, 0));
    assert_eq!(fake.conversions.load(Ordering::SeqCst), 1, "the stick's copy is reused, not encoded again");
}

#[test]
fn turning_conversion_off_puts_the_original_back() {
    let src = tempfile::tempdir().unwrap();
    let dest = common::stick();
    let tracks = vec![track(src.path(), 1, "First.wav")];
    let fake = Fake::default();
    let converting = ExportOptions { converter: Some(&fake), ..Default::default() };
    export_with_options(dest.path(), &tracks, &[], &[], &converting, &mut |_| {}).unwrap();
    let mp3 = entry(&Manifest::load(dest.path()).unwrap(), 1).audio.clone();

    let plain = ExportOptions::default();
    let review = rbl_export::review::inspect_volume(dest.path(), &tracks, &[], &[], &plain).unwrap().plan.unwrap();
    assert!(!review.up_to_date);
    assert_eq!((review.copy, review.convert), (1, 0));
    export_with_options(dest.path(), &tracks, &[], &[], &plain, &mut |_| {}).unwrap();
    let manifest = Manifest::load(dest.path()).unwrap();
    let back = entry(&manifest, 1);
    assert!(back.audio.ends_with("/First.wav") && back.conversion.is_empty() && back.conversion_info.is_none());
    assert_eq!(grid(dest.path(), back), [75]);
    assert!(!dest.path().join(mp3.trim_start_matches('/')).exists());

    let review = rbl_export::review::inspect_volume(dest.path(), &tracks, &[], &[], &converting).unwrap().plan.unwrap();
    assert_eq!((review.copy, review.convert), (0, 1));
}

#[test]
fn a_converter_and_compatibility_together_are_refused() {
    let src = tempfile::tempdir().unwrap();
    let dest = common::stick();
    let fake = Fake::default();
    let options = ExportOptions { converter: Some(&fake), compatibility: Some(rbl_export::CompatibilityFormat::Wav), ..Default::default() };
    let error = export_with_options(dest.path(), &[track(src.path(), 1, "First.wav")], &[], &[], &options, &mut |_| {}).unwrap_err();
    assert!(matches!(error, rbl_export::ExportError::Conflict(_)), "{error}");
}

fn row_size(dest: &Path, audio: &str) -> u32 {
    let bytes = std::fs::read(dest.join("PIONEER/rekordbox/export.pdb")).unwrap();
    let pdb = rbl_pdb::Pdb::parse(&bytes).unwrap();
    pdb.track_rows(pdb.table(rbl_pdb::PageType::Tracks).unwrap()).iter().find(|r| r.file_path == audio).unwrap().file_size
}

#[test]
fn a_converted_name_never_lands_on_another_file() {
    let src = tempfile::tempdir().unwrap();
    let dest = common::stick();
    let tracks = vec![track(src.path(), 1, "First.wav")];
    export_with_options(dest.path(), &tracks, &[], &[], &ExportOptions::default(), &mut |_| {}).unwrap();
    let wav = entry(&Manifest::load(dest.path()).unwrap(), 1).audio.clone();
    let theirs = dest.path().join(wav.trim_start_matches('/')).with_extension("mp3");
    std::fs::write(&theirs, b"someone else's").unwrap();

    let fake = Fake::default();
    let options = ExportOptions { converter: Some(&fake), ..Default::default() };
    export_with_options(dest.path(), &tracks, &[], &[], &options, &mut |_| {}).unwrap();
    let converted = entry(&Manifest::load(dest.path()).unwrap(), 1).audio.clone();
    assert!(converted.ends_with(&format!("/First-{}.mp3", entry(&Manifest::load(dest.path()).unwrap(), 1).export_id)), "{converted}");
    assert_eq!(std::fs::read(&theirs).unwrap(), b"someone else's");

    export_with_options(dest.path(), &tracks, &[], &[], &options, &mut |_| {}).unwrap();
    assert_eq!(entry(&Manifest::load(dest.path()).unwrap(), 1).audio, converted, "the name stays put");
    assert_eq!(fake.conversions.load(Ordering::SeqCst), 1);
}

#[test]
fn a_moved_conversion_keeps_its_own_size() {
    let src = tempfile::tempdir().unwrap();
    let dest = common::stick();
    let mut tracks = vec![track(src.path(), 1, "First.wav")];
    let fake = Fake::default();
    let options = ExportOptions { converter: Some(&fake), ..Default::default() };
    export_with_options(dest.path(), &tracks, &[], &[], &options, &mut |_| {}).unwrap();
    tracks[0].artist = "Renamed".into();
    export_with_options(dest.path(), &tracks, &[], &[], &options, &mut |_| {}).unwrap();
    assert_eq!(fake.conversions.load(Ordering::SeqCst), 1, "moved, not converted again");
    let moved = entry(&Manifest::load(dest.path()).unwrap(), 1).audio.clone();
    assert!(moved.contains("Renamed"), "{moved}");
    let on_stick = std::fs::metadata(dest.path().join(moved.trim_start_matches('/'))).unwrap().len();
    assert_eq!(u64::from(row_size(dest.path(), &moved)), on_stick);
}

#[test]
fn a_full_stick_while_converting_stops_the_export() {
    let src = tempfile::tempdir().unwrap();
    let dest = common::stick();
    let fake = Fake { fail: Some(std::io::ErrorKind::StorageFull), ..Default::default() };
    let options = ExportOptions { converter: Some(&fake), ..Default::default() };
    assert!(export_with_options(dest.path(), &[track(src.path(), 1, "First.wav")], &[], &[], &options, &mut |_| {}).is_err());
}

#[test]
fn a_library_file_on_the_stick_is_reviewed_as_a_conversion() {
    let dest = common::stick();
    let music = dest.path().join("Music");
    std::fs::create_dir_all(&music).unwrap();
    let tracks = vec![track(&music, 1, "First.wav")];
    let fake = Fake::default();
    let options = ExportOptions { converter: Some(&fake), ..Default::default() };
    let plan = rbl_export::review::inspect_volume(dest.path(), &tracks, &[], &[], &options).unwrap().plan.unwrap();
    assert_eq!((plan.convert, plan.keep), (1, 0));
}
