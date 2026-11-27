//! Where a sync writes each file.
//!
//! The journal keeps the previous library whole until the commit, but every file it carries is renamed into
//! place and its folder flushed one at a time, which on a first sync is most of the time spent publishing.
//! A file at a path that holds nothing, and that the stick's databases don't name, can't be part of the
//! previous library, so it is written straight to its place and flushed with the others just before the
//! commit. Anything else goes through the stage and the journal as before.

use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::snapshot::Snapshot;
use crate::{path_key, under, Manifest};

/// Lists the files a run has written in place, so a run that stops before its commit can be cleaned up by the
/// next one. Appended without flushing: it reaches the stick with the files it names.
const IN_PLACE_RECORD: &str = ".exportstudio-in-place";

/// Names a copy before it is complete, so one a crash leaves behind can be told apart from anyone else's file.
pub(crate) const PARTIAL_PREFIX: &str = ".exportstudio-partial-";

pub(crate) struct Placement {
    destination: PathBuf,
    stage: PathBuf,
    named: Names,
    in_place: Vec<String>,
    record: Option<std::fs::File>,
    committed: bool,
}

impl Placement {
    pub(crate) fn new(destination: &Path, stage: &Path, before: &Snapshot) -> Self {
        Self {
            destination: destination.to_owned(),
            stage: stage.to_owned(),
            named: Names::of(before),
            in_place: Vec::new(),
            record: None,
            committed: false,
        }
    }

    /// Where to write `relative`, with its folder made.
    pub(crate) fn target(&mut self, relative: &str) -> std::io::Result<PathBuf> {
        let in_place = under(&self.destination, relative);
        let target = if in_place.exists() || self.named.contains(relative) {
            under(&self.stage, relative)
        } else {
            self.remember(relative)?;
            in_place
        };
        if let Some(parent) = target.parent() { std::fs::create_dir_all(parent)?; }
        Ok(target)
    }

    fn remember(&mut self, relative: &str) -> std::io::Result<()> {
        if self.record.is_none() {
            self.record = Some(std::fs::OpenOptions::new().create(true).append(true).open(self.destination.join(IN_PLACE_RECORD))?);
        }
        if let Some(record) = &mut self.record { writeln!(record, "{}", relative.trim_start_matches('/'))?; }
        self.in_place.push(relative.to_owned());
        Ok(())
    }

    /// From here on the commit owns these files: a commit that fails part way may still be replayed onto them.
    pub(crate) fn make_durable_for_commit(&mut self) -> std::io::Result<()> {
        self.committed = true;
        crate::incremental::record(|metrics| metrics.files_written_in_place += self.in_place.len() as u64);
        let mut folders = BTreeSet::new();
        for relative in &self.in_place {
            let path = under(&self.destination, relative);
            match std::fs::OpenOptions::new().write(true).open(&path) {
                Ok(file) => rbl_core::durable::flush(&file)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => return Err(e),
            }
            folders.extend(path.ancestors().skip(1).take_while(|folder| *folder != self.destination).map(Path::to_path_buf));
        }
        for folder in folders.iter().rev() { rbl_core::durable::sync_dir(folder)?; }
        if !folders.is_empty() { rbl_core::durable::sync_dir(&self.destination)?; }
        Ok(())
    }

    pub(crate) fn published(mut self) {
        self.record = None;
        let _ = std::fs::remove_file(self.destination.join(IN_PLACE_RECORD));
    }
}

impl Drop for Placement {
    fn drop(&mut self) {
        if self.committed { return; }
        self.record = None;
        remove_in_place(&self.destination, self.in_place.iter().map(String::as_str), &Names::default());
    }
}

/// Removes what a run that stopped before its commit wrote in place, sparing anything the stick's library now names.
pub(crate) fn clean_up_interrupted_run(destination: &Path) {
    let Ok(record) = std::fs::read_to_string(destination.join(IN_PLACE_RECORD)) else { return };
    let mut named = Snapshot::read(destination).map(|live| Names::of(&live)).unwrap_or_default();
    if let Some(manifest) = Manifest::load(destination) { named.add_manifest(&manifest, destination); }
    remove_in_place(destination, record.lines().filter(|line| !line.is_empty()), &named);
}

fn remove_in_place<'a>(destination: &Path, relatives: impl Iterator<Item = &'a str>, spared: &Names) {
    let mut folders = BTreeSet::new();
    for relative in relatives.filter(|relative| !spared.contains(relative)) {
        let path = under(destination, relative);
        let _ = std::fs::remove_file(&path);
        if let Some(folder) = path.parent() { folders.insert(folder.to_owned()); }
    }
    for folder in &folders { remove_partial_copies(folder); }
    for folder in folders.iter().rev() {
        let mut empty = Some(folder.as_path());
        while let Some(dir) = empty.filter(|dir| *dir != destination && dir.starts_with(destination)) {
            if std::fs::remove_dir(dir).is_err() { break; }
            empty = dir.parent();
        }
    }
    let _ = std::fs::remove_file(destination.join(IN_PLACE_RECORD));
}

fn remove_partial_copies(folder: &Path) {
    let Ok(entries) = std::fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().starts_with(PARTIAL_PREFIX) { let _ = std::fs::remove_file(entry.path()); }
    }
}

/// Files a library names: its audio, and every file in its analysis folders, since a player reads the
/// EXT and 2EX beside the DAT a track names.
#[derive(Default)]
struct Names {
    files: BTreeSet<String>,
    folders: BTreeSet<String>,
}

impl Names {
    fn of(snapshot: &Snapshot) -> Self {
        let mut names = Self::default();
        for track in [&snapshot.legacy, &snapshot.one].into_iter().flatten().flat_map(|library| &library.tracks) {
            names.files.insert(key(&track.path));
            if let Some((folder, _)) = track.analysis.rsplit_once('/') { names.folders.insert(key(folder)); }
        }
        names
    }

    fn add_manifest(&mut self, manifest: &Manifest, destination: &Path) {
        let root_name = crate::export_root_name(destination).unwrap_or("PIONEER");
        for track in &manifest.tracks {
            self.files.insert(key(&track.audio));
            self.folders.insert(key(&track.anlz_dir));
            let artwork = crate::artwork_id_from_path(&track.artwork);
            if artwork != 0 {
                for (name, _) in crate::artwork_names(artwork) {
                    self.files.insert(key(&name.replacen("/PIONEER/", &format!("/{root_name}/"), 1)));
                }
            }
        }
    }

    fn contains(&self, relative: &str) -> bool {
        let relative = key(relative);
        self.files.contains(&relative) || relative.rsplit_once('/').is_some_and(|(folder, _)| self.folders.contains(folder))
    }
}

fn key(path: &str) -> String {
    path_key(path.trim_start_matches('/'))
}
