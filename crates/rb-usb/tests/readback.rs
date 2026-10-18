//! Exports through the public SDK service and reads both device formats back.

use std::path::{Path, PathBuf};

use rb_device::{read::read, read::read_kind, LibraryKind};
use rb_model::source::{Cue, Metadata, Playlist, Source, Track};
use rb_usb::{export, Options};

fn wav(path: &Path) {
    let data = 16000u32 * 2;
    let mut bytes = b"RIFF".to_vec();
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend([1, 0, 1, 0]);
    bytes.extend(16000u32.to_le_bytes());
    bytes.extend(32000u32.to_le_bytes());
    bytes.extend([2, 0, 16, 0]);
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    bytes.extend(vec![0u8; data as usize]);
    std::fs::write(path, bytes).unwrap();
}

fn stick() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let source_dir = dir.path().join("source");
    std::fs::create_dir_all(&source_dir).unwrap();
    let artwork = source_dir.join("cover.jpg");
    std::fs::write(&artwork, b"artwork fixture").unwrap();
    let volume = dir.path().join("STICK");
    std::fs::create_dir_all(&volume).unwrap();

    let tracks = [(101, "First", "Alpha"), (102, "Second", "Beta")]
        .into_iter()
        .map(|(id, title, artist)| {
            let source_path = source_dir.join(format!("{title}.wav"));
            wav(&source_path);
            Track {
                id,
                source_path,
                title: title.into(),
                artist: artist.into(),
                album: "Album".into(),
                genre: "House".into(),
                bpm_x100: 12800,
                duration_sec: 1,
                rating: 3,
                year: 2020,
                bitrate: 512,
                sample_rate: 16000,
                file_size: 32044,
                metadata: Metadata {
                    track_number: 2,
                    bit_depth: 16,
                    analysed: 105,
                    ..Default::default()
                },
                artwork: (id == 101).then(|| artwork.clone()),
                cues: Some(vec![
                    Cue {
                        kind: 0,
                        time_ms: 500,
                        ..Default::default()
                    },
                    Cue {
                        kind: 1,
                        time_ms: 100,
                        color_code: 10,
                        comment: "drop".into(),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            }
        })
        .collect();
    let source = Source {
        db_id: 4242,
        tracks,
        playlists: vec![
            Playlist {
                id: 1,
                name: "Folder".into(),
                folder: true,
                ..Default::default()
            },
            Playlist {
                id: 2,
                name: "Set".into(),
                parent_id: 1,
                track_ids: vec![102, 101],
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    export(
        &volume,
        &source,
        &Options::default(),
        &mut |_| {},
        &|| false,
    )
    .unwrap();
    (dir, volume)
}

#[test]
fn a_stick_reads_back_with_its_playlists_in_order() {
    let (_dir, volume) = stick();
    let device = read(&volume).unwrap().expect("a library");
    assert_eq!(device.kind, LibraryKind::OneLibrary);
    assert_eq!(device.tracks.len(), 2);
    let first = device.tracks.iter().find(|track| track.title == "First").unwrap();
    assert_eq!(
        (
            first.artist.as_str(),
            first.album.as_str(),
            first.genre.as_str()
        ),
        ("Alpha", "Album", "House")
    );
    assert_eq!((first.bpm_x100, first.sample_rate, first.file_type), (12800, 16000, 11));
    assert_eq!(first.master_id, Some(101));

    let set = device.playlists.iter().find(|playlist| playlist.name == "Set").unwrap();
    let titles: Vec<_> = set
        .track_ids
        .iter()
        .map(|id| device.track(*id).unwrap().title.as_str())
        .collect();
    assert_eq!(titles, ["Second", "First"]);
    let folder = device
        .playlists
        .iter()
        .find(|playlist| playlist.name == "Folder")
        .unwrap();
    assert!(folder.folder);
    assert_eq!(set.parent_id, folder.export_id);
}

#[test]
fn cues_come_from_the_database_with_their_times() {
    let (_dir, volume) = stick();
    let device = read(&volume).unwrap().unwrap();
    let cues = &device.tracks[0].cues;
    let mut summary: Vec<_> = cues
        .iter()
        .map(|cue| (cue.kind, cue.start_ms, cue.comment.as_str()))
        .collect();
    summary.sort_unstable();
    assert_eq!(summary, [(0, 500, ""), (1, 100, "drop")]);
}

#[test]
fn the_older_database_reads_the_same_library() {
    let (_dir, volume) = stick();
    let one = read_kind(&volume, LibraryKind::OneLibrary).unwrap().unwrap();
    let old = read_kind(&volume, LibraryKind::DeviceLibrary)
        .unwrap()
        .expect("export.pdb is written too");
    let titles = |device: &rb_device::Device| {
        let mut tracks: Vec<_> = device
            .tracks
            .iter()
            .map(|track| {
                (
                    track.title.clone(),
                    track.artist.clone(),
                    track.bpm_x100,
                )
            })
            .collect();
        tracks.sort();
        tracks
    };
    assert_eq!(titles(&old), titles(&one));
    let set = old
        .playlists
        .iter()
        .find(|playlist| playlist.name == "Set")
        .unwrap();
    let names: Vec<_> = set
        .track_ids
        .iter()
        .map(|id| old.track(*id).unwrap().title.as_str())
        .collect();
    assert_eq!(names, ["Second", "First"]);
    let first = old
        .tracks
        .iter()
        .find(|track| track.title == "First")
        .unwrap();
    assert_eq!(first.master_id, Some(101));
}

#[test]
fn both_device_libraries_keep_their_artwork_paths() {
    let (_dir, volume) = stick();
    for kind in [LibraryKind::OneLibrary, LibraryKind::DeviceLibrary] {
        let device = read_kind(&volume, kind).unwrap().unwrap();
        let first = device
            .tracks
            .iter()
            .find(|track| track.title == "First")
            .unwrap();
        assert!(!first.artwork.is_empty(), "missing artwork in {kind:?}");
        let path = rb_device::paths::volume_path(&first.artwork, &volume).unwrap();
        assert_eq!(std::fs::read(path).unwrap(), b"artwork fixture");
        let second = device
            .tracks
            .iter()
            .find(|track| track.title == "Second")
            .unwrap();
        assert!(second.artwork.is_empty());
    }
}
