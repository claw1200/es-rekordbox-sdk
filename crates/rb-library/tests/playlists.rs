use rb_library::fixture::{self, Rows};
use rb_library::{Error, Guard, Library, PlaylistKind};
use tempfile::TempDir;

const XML: &str = "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<MASTER_PLAYLIST Version=\"3.0.0\" AutomaticSync=\"0\">\n    <PLAYLISTS>\n        <NODE Id=\"A\" ParentId=\"0\" Attribute=\"0\" Timestamp=\"1\" Lib_Type=\"0\" CheckType=\"0\"/>\n        <NODE Id=\"B\" ParentId=\"0\" Attribute=\"1\" Timestamp=\"1\" Lib_Type=\"0\" CheckType=\"0\"/>\n    </PLAYLISTS>\n</MASTER_PLAYLIST>\n";

struct Scene {
    dir: TempDir,
    lib: Library,
}

fn scene() -> Scene {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture::build(dir.path(), 50).unwrap();
    std::fs::write(dir.path().join("masterPlaylists6.xml"), XML).unwrap();
    {
        let conn = fixture::connect(&path).unwrap();
        let rows = Rows(&conn);
        for id in ["1", "2", "3", "4"] {
            rows.track(id, id).unwrap();
        }
        // Ids 10 and 11 are hex A and B, as the XML above names them.
        rows.playlist("10", "Mix", 0).unwrap();
        rows.playlist("11", "Folder", 1).unwrap();
        for (n, id) in ["1", "2", "3", "4"].iter().enumerate() {
            rows.song("10", id, i64::try_from(n).unwrap() + 1).unwrap();
        }
        conn.execute("INSERT INTO djmdMyTag (ID, Name, Attribute, ParentID, Seq) VALUES ('t1', 'Tag', 0, 'root', 1)", [])
            .unwrap();
        conn.execute("INSERT INTO djmdMyTag (ID, Name, Attribute, ParentID, Seq) VALUES ('t2', 'Dir', 1, 'root', 2)", [])
            .unwrap();
        conn.execute("UPDATE djmdPlaylist SET Seq = 1 WHERE ID = '10'", []).unwrap();
        conn.execute("UPDATE djmdPlaylist SET Seq = 2 WHERE ID = '11'", []).unwrap();
    }
    let lib = Library::open_live(&path, Guard::Off).unwrap();
    Scene { dir, lib }
}

fn xml(s: &Scene) -> String {
    std::fs::read_to_string(s.dir.path().join("masterPlaylists6.xml")).unwrap()
}

#[test]
fn creating_a_playlist_puts_it_last_and_lists_it_in_the_xml() {
    let mut s = scene();
    let id = s.lib.write(|tx| tx.create_playlist("New", None, false, &["1".into(), "2".into()])).unwrap();
    let nodes = s.lib.playlists().unwrap();
    let made = nodes.iter().find(|n| n.id == id).unwrap();
    assert_eq!((made.seq, made.kind, made.count), (3, PlaylistKind::Playlist, Some(2)));
    let hex = format!("{:X}", id.parse::<u64>().unwrap());
    assert!(xml(&s).contains(&format!("<NODE Id=\"{hex}\" ParentId=\"0\" Attribute=\"0\"")));
}

#[test]
fn a_playlist_only_goes_inside_a_folder() {
    let mut s = scene();
    let inside = s.lib.write(|tx| tx.create_playlist("In", Some("11"), false, &[])).unwrap();
    assert_eq!(s.lib.playlists().unwrap().iter().find(|n| n.id == inside).unwrap().parent_id, "11");
    let bad = s.lib.write(|tx| tx.create_playlist("Bad", Some("10"), false, &[]));
    assert!(matches!(bad, Err(Error::Refused(_))));
}

#[test]
fn moving_playlists_reorders_and_updates_folder_parent() {
    let mut s = scene();
    let inner = s.lib.write(|tx| tx.create_playlist("Inside", Some("11"), false, &[])).unwrap();
    let after = s.lib.write(|tx| tx.create_playlist("After", None, false, &[])).unwrap();

    s.lib.write(|tx| tx.move_playlist("10", "11", 0)).unwrap();
    let nodes = s.lib.playlists().unwrap();
    let moved = nodes.iter().find(|node| node.id == "10").unwrap();
    assert_eq!((moved.parent_id.as_str(), moved.seq), ("11", 1));
    assert_eq!(nodes.iter().find(|node| node.id == inner).unwrap().seq, 2);
    assert!(xml(&s).contains("<NODE Id=\"A\" ParentId=\"B\""));

    s.lib.write(|tx| tx.move_playlist(&after, "root", 0)).unwrap();
    let nodes = s.lib.playlists().unwrap();
    assert_eq!(nodes.iter().find(|node| node.id == after).unwrap().seq, 1);
    assert_eq!(nodes.iter().find(|node| node.id == "11").unwrap().seq, 2);
}

#[test]
fn moving_root_playlists_ignores_hidden_row_and_null_parents() {
    let mut s = scene();
    let hidden = "100000";
    let before: (Option<i64>, Option<i64>, Option<String>) = {
        let conn = fixture::connect(s.lib.path()).unwrap();
        Rows(&conn).playlist(hidden, "Cloud sync", 0).unwrap();
        conn.execute("UPDATE djmdPlaylist SET ParentID = NULL WHERE ID IN ('10', '11', ?1)", [hidden]).unwrap();
        conn.execute("UPDATE djmdPlaylist SET Seq = 0 WHERE ID = ?1", [hidden]).unwrap();
        conn.query_row(
            "SELECT rb_local_usn, Seq, ParentID FROM djmdPlaylist WHERE ID = ?1",
            [hidden],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    };

    s.lib.write(|tx| tx.move_playlist("11", "root", 0)).unwrap();

    let mut visible: Vec<_> = s.lib.playlists().unwrap().into_iter().filter(|node| node.parent_id == "root").collect();
    visible.sort_by_key(|node| node.seq);
    assert_eq!(visible.iter().map(|node| node.id.as_str()).collect::<Vec<_>>(), ["11", "10"]);
    let conn = fixture::connect(s.lib.path()).unwrap();
    let after: (Option<i64>, Option<i64>, Option<String>) = conn
        .query_row(
            "SELECT rb_local_usn, Seq, ParentID FROM djmdPlaylist WHERE ID = ?1",
            [hidden],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(after, before);
}

#[test]
fn moving_a_folder_into_its_descendant_or_a_playlist_is_refused() {
    let mut s = scene();
    let inner_folder = s.lib.write(|tx| tx.create_playlist("Inner folder", Some("11"), true, &[])).unwrap();

    assert!(matches!(s.lib.write(|tx| tx.move_playlist("11", &inner_folder, 0)), Err(Error::Refused(_))));
    assert!(matches!(s.lib.write(|tx| tx.move_playlist("10", "10", 0)), Err(Error::Refused(_))));
}

#[test]
fn renaming_changes_the_name_and_takes_an_update_count() {
    let mut s = scene();
    s.lib.write(|tx| tx.rename_playlist("10", "Renamed")).unwrap();
    assert_eq!(s.lib.playlists().unwrap().iter().find(|n| n.id == "10").unwrap().name, "Renamed");
    let conn = fixture::connect(s.lib.path()).unwrap();
    let usn: i64 = conn.query_row("SELECT rb_local_usn FROM djmdPlaylist WHERE ID = '10'", [], |r| r.get(0)).unwrap();
    assert_eq!(usn, 51);
    assert!(!xml(&s).contains("<NODE Id=\"A\" ParentId=\"0\" Attribute=\"0\" Timestamp=\"1\""));
}

#[test]
fn deleting_a_folder_takes_what_is_inside_and_closes_the_gap() {
    let mut s = scene();
    let inside = s.lib.write(|tx| tx.create_playlist("In", Some("11"), false, &["1".into()])).unwrap();
    let after = s.lib.write(|tx| tx.create_playlist("After", None, false, &[])).unwrap();
    s.lib.write(|tx| tx.delete_playlist("11")).unwrap();

    let nodes = s.lib.playlists().unwrap();
    assert!(nodes.iter().all(|n| n.id != "11" && n.id != inside));
    assert_eq!(nodes.iter().find(|n| n.id == after).unwrap().seq, 2);
    let conn = fixture::connect(s.lib.path()).unwrap();
    let orphans: i64 =
        conn.query_row("SELECT count(*) FROM djmdSongPlaylist WHERE PlaylistID = ?1", [&inside], |r| r.get(0)).unwrap();
    assert_eq!(orphans, 0);
    assert!(!xml(&s).contains("Id=\"B\""));
}

#[test]
fn moving_tracks_keeps_them_together_and_numbers_stay_contiguous() {
    let mut s = scene();
    s.lib.write(|tx| tx.move_in_playlist("10", &["3".into(), "4".into()], 0)).unwrap();
    assert_eq!(s.lib.playlist_track_ids("10").unwrap(), ["3", "4", "1", "2"]);
    s.lib.write(|tx| tx.move_in_playlist("10", &["3".into()], 4)).unwrap();
    assert_eq!(s.lib.playlist_track_ids("10").unwrap(), ["4", "1", "2", "3"]);
}

#[test]
fn removing_from_a_playlist_renumbers() {
    let mut s = scene();
    s.lib.write(|tx| tx.remove_from_playlist("10", &["2".into()])).unwrap();
    assert_eq!(s.lib.playlist_track_ids("10").unwrap(), ["1", "3", "4"]);
}

#[test]
fn tagging_adds_once_and_untagging_removes() {
    let mut s = scene();
    assert_eq!(s.lib.write(|tx| tx.add_tag("t1", &["1".into(), "2".into()])).unwrap(), 2);
    assert_eq!(s.lib.write(|tx| tx.add_tag("t1", &["2".into(), "3".into()])).unwrap(), 1);
    assert_eq!(s.lib.my_tag_ids(None).unwrap().len(), 3);
    assert_eq!(s.lib.write(|tx| tx.remove_tag("t1", &["1".into()])).unwrap(), 1);
    assert!(s.lib.my_tag_ids(Some(&["1".into()])).unwrap().is_empty());
    assert!(matches!(s.lib.write(|tx| tx.add_tag("t2", &["1".into()])), Err(Error::Refused(_))));
}

#[test]
fn removed_tracks_lose_their_analysis_columns_and_report_the_files() {
    let mut s = scene();
    {
        let conn = fixture::connect(s.lib.path()).unwrap();
        conn.execute("UPDATE djmdContent SET AnalysisDataPath = '/PIONEER/a/ANLZ0000.DAT', Analysed = 105 WHERE ID = '1'", [])
            .unwrap();
    }
    let removed = s.lib.write(|tx| tx.remove_tracks(&["1".into(), "2".into()])).unwrap();
    assert_eq!(removed[0].analysis_path.as_deref(), Some("/PIONEER/a/ANLZ0000.DAT"));
    assert_eq!(removed[1].analysis_path, None);
    let conn = fixture::connect(s.lib.path()).unwrap();
    let analysed: Option<i64> = conn.query_row("SELECT Analysed FROM djmdContent WHERE ID = '1'", [], |r| r.get(0)).unwrap();
    assert_eq!(analysed, Some(0));
}
