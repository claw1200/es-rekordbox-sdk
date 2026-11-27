//! A stick another writer has also exported this library to: one row per library track afterwards, its copies
//! reused where they are what the sync writes, and nothing left twice.
#![allow(clippy::pedantic, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::path::{Path, PathBuf};

use rbl_export::snapshot::Snapshot;
use rbl_export::{
    export_with_options, AudioConverter, ConvertedAudio, DeviceTrack, ExportOptions, SourcePlaylist, SourceTrack,
    SyncSource,
};

const DB_ID: u64 = 4_242;

/// Converts anything that isn't an MP3 by appending a marker.
struct ToMp3;

impl AudioConverter for ToMp3 {
    fn profile(&self) -> &str { "to-mp3-v1" }
    fn extension(&self) -> &str { "mp3" }
    fn wants(&self, source: &Path) -> std::io::Result<bool> { Ok(source.extension().is_none_or(|e| e != "mp3")) }
    fn convert(&self, source: &Path, destination: &Path) -> std::io::Result<ConvertedAudio> {
        std::fs::write(destination, [std::fs::read(source)?, b"-mp3".to_vec()].concat())?;
        Ok(ConvertedAudio { offset_ms: 0, bitrate: 320, sample_rate: 44_100, bit_depth: 0 })
    }
    fn shift_analysis(&self, _: &str, bytes: &[u8], _: i64) -> std::io::Result<Vec<u8>> { Ok(bytes.to_vec()) }
    fn estimate_bytes(&self, duration_sec: u64) -> u64 { duration_sec * 40_000 }
}

fn library_track(src: &Path, id: u64) -> SourceTrack {
    let path = src.join(format!("Track {id}.wav"));
    std::fs::write(&path, vec![id as u8; 4096]).unwrap();
    SourceTrack {
        id,
        source_path: path,
        title: format!("Track {id}"),
        artist: "Artist".into(),
        album: "Album".into(),
        duration_sec: 60,
        analysis: vec![("DAT".into(), rbl_anlz::AnlzBuilder::new().path("/x").finish())],
        ..Default::default()
    }
}

/// Another writer's copy of `track`, placed where an unconverted sync of it would put the audio, with its own
/// analysis folder, as rekordbox leaves one after exporting the same library.
fn other_writers_copy(dest: &Path, track: &SourceTrack) -> SourceTrack {
    let audio = format!("/Contents/Artist/Album/Track {}.wav", track.id);
    let analysis_dir = format!("/PIONEER/USBANLZ/P0FF/{:08X}", track.id);
    let file = dest.join(audio.trim_start_matches('/'));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::copy(&track.source_path, &file).unwrap();
    let anlz = dest.join(analysis_dir.trim_start_matches('/'));
    std::fs::create_dir_all(&anlz).unwrap();
    std::fs::write(anlz.join("ANLZ0000.DAT"), rbl_anlz::AnlzBuilder::new().path(&audio).finish()).unwrap();
    SourceTrack {
        source_path: file,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: track.album.clone(),
        duration_sec: track.duration_sec,
        analysis_source: Some(anlz.join("ANLZ0000.DAT")),
        device: Some(DeviceTrack {
            id: 100 + u32::try_from(track.id).unwrap(),
            master_db_id: DB_ID,
            master_content_id: track.id,
            audio,
            analysis_dir,
            preserve: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn sync(dest: &Path, tracks: &[SourceTrack], playlists: &[SourcePlaylist], converter: Option<&dyn AudioConverter>) -> rbl_export::Result<rbl_export::ExportReport> {
    let source = SyncSource { db_id: DB_ID, ..Default::default() };
    export_with_options(dest, tracks, playlists, &[], &ExportOptions { sync: Some(&source), converter, ..Default::default() }, &mut |_| {})
}

fn audio_files(dest: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap().flatten().filter(|e| !common::is_os_metadata(&e.file_name())) {
            if entry.path().is_dir() { walk(&entry.path(), out) } else { out.push(entry.path()) }
        }
    }
    let mut out = Vec::new();
    walk(&dest.join("Contents"), &mut out);
    out.sort();
    out
}

/// What BACKUP1 held: this library's tracks synced converted, then the same tracks exported unconverted by
/// another writer, so each library track has two rows on the stick.
fn stick_with_both_copies(src: &Path, dest: &Path) -> (Vec<SourceTrack>, Vec<SourcePlaylist>) {
    let tracks = vec![library_track(src, 1), library_track(src, 2)];
    let playlists = vec![SourcePlaylist { id: 10, name: "Set".into(), track_indices: vec![0, 1], ..Default::default() }];
    let mut both = tracks.clone();
    both.extend(tracks.iter().map(|track| other_writers_copy(dest, track)));
    sync(dest, &both, &playlists, Some(&ToMp3)).unwrap();
    assert_eq!(Snapshot::read(dest).unwrap().legacy.unwrap().tracks.len(), 4, "the stick starts with two rows per track");
    (tracks, playlists)
}

#[test]
fn a_sync_without_conversion_reuses_the_other_writers_copies_and_drops_its_own() {
    let (src, dest) = (tempfile::tempdir().unwrap(), common::stick());
    let (tracks, playlists) = stick_with_both_copies(src.path(), dest.path());

    let report = sync(dest.path(), &tracks, &playlists, None).unwrap();

    assert_eq!(report.bytes_copied, 0, "the other writer's copies are what this sync writes");
    let library = Snapshot::read(dest.path()).unwrap().legacy.unwrap();
    assert_eq!(library.tracks.len(), 2, "one row per library track");
    let files = audio_files(dest.path());
    assert_eq!(files.len(), 2, "{files:?}");
    assert!(files.iter().all(|file| file.extension().is_some_and(|e| e == "wav")), "the converted copies are gone: {files:?}");
    assert!(!dest.path().join("PIONEER/USBANLZ/P0FF").exists(), "the other writer's analysis goes with its rows");
    assert!(rbl_export::verify(dest.path()).unwrap().is_ok());
}

#[test]
fn a_converted_sync_keeps_its_own_copies_and_drops_the_other_writers() {
    let (src, dest) = (tempfile::tempdir().unwrap(), common::stick());
    let (tracks, playlists) = stick_with_both_copies(src.path(), dest.path());

    let report = sync(dest.path(), &tracks, &playlists, Some(&ToMp3)).unwrap();

    assert_eq!(report.bytes_copied, 0);
    assert_eq!(Snapshot::read(dest.path()).unwrap().legacy.unwrap().tracks.len(), 2);
    let files = audio_files(dest.path());
    assert!(files.iter().all(|file| file.extension().is_some_and(|e| e == "mp3")), "{files:?}");
    assert!(rbl_export::verify(dest.path()).unwrap().is_ok());
}

#[test]
fn another_writers_copy_that_differs_from_the_library_is_kept() {
    let (src, dest) = (tempfile::tempdir().unwrap(), common::stick());
    let (tracks, playlists) = stick_with_both_copies(src.path(), dest.path());
    let theirs = dest.path().join("Contents/Artist/Album/Track 1.wav");
    std::fs::write(&theirs, vec![99; 4096]).unwrap();

    sync(dest.path(), &tracks, &playlists, Some(&ToMp3)).unwrap();

    assert!(theirs.is_file(), "a copy that isn't the library's file is never deleted");
    assert!(rbl_export::verify(dest.path()).unwrap().is_ok());
}

#[test]
fn a_playlist_another_writer_exported_is_taken_over_not_duplicated() {
    let (src, dest) = (tempfile::tempdir().unwrap(), common::stick());
    let tracks = vec![library_track(src.path(), 1), library_track(src.path(), 2)];
    let theirs: Vec<SourceTrack> = tracks.iter().map(|track| other_writers_copy(dest.path(), track)).collect();
    let their_playlist = SourcePlaylist { id: 99, name: "Set".into(), device_only: true, track_indices: vec![0, 1], ..Default::default() };
    sync(dest.path(), &theirs, &[their_playlist], None).unwrap();

    let playlists = vec![SourcePlaylist { id: 10, name: "Set".into(), track_indices: vec![0, 1], ..Default::default() }];
    let report = sync(dest.path(), &tracks, &playlists, None).unwrap();

    assert_eq!(report.bytes_copied, 0);
    let library = Snapshot::read(dest.path()).unwrap().legacy.unwrap();
    assert_eq!(library.playlists.iter().filter(|p| p.name == "Set").count(), 1, "{:?}", library.playlists);
    assert_eq!(library.tracks.len(), 2);
    assert!(rbl_export::verify(dest.path()).unwrap().is_ok());
}
