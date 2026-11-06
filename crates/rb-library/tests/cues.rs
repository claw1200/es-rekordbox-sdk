use rb_library::fixture::{self, Rows};
use rb_library::{Error, Guard, Library};
use tempfile::TempDir;

fn scene() -> (TempDir, Library) {
    let dir = tempfile::tempdir().unwrap();
    let path = fixture::build(dir.path(), 10).unwrap();
    {
        let conn = fixture::connect(&path).unwrap();
        conn.execute("INSERT INTO djmdContent (ID, Title, UUID, Length) VALUES ('1', 'One', 'u-1', 200)", []).unwrap();
        Rows(&conn).playlist("10", "Mix", 0).unwrap();
    }
    (dir, Library::open_live(&path, Guard::Off).unwrap())
}

fn cues(lib: &Library) -> Vec<(i64, i64)> {
    let mut out: Vec<_> = lib.cues("1").unwrap().into_iter().map(|c| (c.kind, c.start_ms)).collect();
    out.sort_unstable();
    out
}

#[test]
fn a_hot_cue_replaces_the_one_in_its_slot_and_is_clamped_to_the_track() {
    let (_dir, mut lib) = scene();
    lib.write(|tx| tx.set_hot_cue("1", 2, 1000, None)).unwrap();
    lib.write(|tx| tx.set_hot_cue("1", 2, 5000, None)).unwrap();
    lib.write(|tx| tx.set_hot_cue("1", 3, 999_999, None)).unwrap();
    assert_eq!(cues(&lib), [(2, 5000), (3, 200_000)]);
}

#[test]
fn loops_and_unknown_slots_are_not_hot_cue_slots() {
    let (_dir, mut lib) = scene();
    assert!(matches!(lib.write(|tx| tx.set_hot_cue("1", 4, 0, None)), Err(Error::Refused(_))));
    assert!(matches!(lib.write(|tx| tx.set_hot_cue("1", 10, 0, None)), Err(Error::Refused(_))));
}

#[test]
fn a_memory_cue_is_added_once_per_time() {
    let (_dir, mut lib) = scene();
    for _ in 0..2 {
        lib.write(|tx| tx.add_memory_cue("1", 2500, None)).unwrap();
    }
    lib.write(|tx| tx.add_memory_cue("1", 3000, None)).unwrap();
    assert_eq!(cues(&lib), [(0, 2500), (0, 3000)]);
}

#[test]
fn deleting_and_colouring_cues() {
    let (_dir, mut lib) = scene();
    lib.write(|tx| tx.set_hot_cue("1", 1, 100, None)).unwrap();
    lib.write(|tx| tx.add_memory_cue("1", 200, None)).unwrap();
    let all = lib.cues("1").unwrap();
    let hot = all.iter().find(|c| c.kind == 1).unwrap().id.clone();
    let memory = all.iter().find(|c| c.kind == 0).unwrap().id.clone();

    lib.write(|tx| tx.set_cue_color("1", &hot, Some(12))).unwrap();
    assert_eq!(lib.cues("1").unwrap().iter().find(|c| c.kind == 1).unwrap().color_index, Some(12));
    assert!(matches!(lib.write(|tx| tx.set_cue_color("1", &hot, Some(63))), Err(Error::Refused(_))));
    assert!(matches!(lib.write(|tx| tx.set_cue_color("1", &memory, Some(1))), Err(Error::NotFound(_))));

    lib.write(|tx| tx.delete_cue("1", &memory)).unwrap();
    assert_eq!(cues(&lib), [(1, 100)]);
}

#[test]
fn a_new_cue_carries_the_frame_position_and_the_track_uuid() {
    let (_dir, mut lib) = scene();
    lib.write(|tx| tx.set_hot_cue("1", 1, 2000, None)).unwrap();
    let conn = fixture::connect(lib.path()).unwrap();
    let (frame, uuid, out): (i64, String, i64) = conn
        .query_row("SELECT InFrame, ContentUUID, OutMsec FROM djmdCue", [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap();
    assert_eq!((frame, uuid.as_str(), out), (300, "u-1", -1));
}

fn ranges(lib: &Library) -> Vec<(i64, i64, Option<i64>)> {
    let mut out: Vec<_> = lib.cues("1").unwrap().into_iter().map(|c| (c.kind, c.start_ms, c.end_ms)).collect();
    out.sort_unstable();
    out
}

#[test]
fn a_hot_cue_can_be_a_loop_clamped_to_the_track() {
    let (_dir, mut lib) = scene();
    lib.write(|tx| tx.set_hot_cue("1", 1, 1000, Some(3000))).unwrap();
    lib.write(|tx| tx.set_hot_cue("1", 2, 199_000, Some(999_999))).unwrap();
    assert_eq!(ranges(&lib), [(1, 1000, Some(3000)), (2, 199_000, Some(200_000))]);

    let conn = fixture::connect(lib.path()).unwrap();
    let frame: i64 = conn.query_row("SELECT OutFrame FROM djmdCue WHERE Kind = 1", [], |r| r.get(0)).unwrap();
    assert_eq!(frame, 450);
}

#[test]
fn a_loop_that_does_not_end_after_its_start_is_a_plain_cue() {
    let (_dir, mut lib) = scene();
    lib.write(|tx| tx.set_hot_cue("1", 1, 2000, Some(2000))).unwrap();
    lib.write(|tx| tx.add_memory_cue("1", 200_000, Some(250_000))).unwrap();
    assert_eq!(ranges(&lib), [(0, 200_000, Some(-1)), (1, 2000, Some(-1))]);
}

#[test]
fn memory_loops_are_added_once_per_range_beside_a_cue_at_their_start() {
    let (_dir, mut lib) = scene();
    for _ in 0..2 {
        lib.write(|tx| tx.add_memory_cue("1", 2500, Some(4500))).unwrap();
        lib.write(|tx| tx.add_memory_cue("1", 2500, None)).unwrap();
    }
    lib.write(|tx| tx.add_memory_cue("1", 2500, Some(6500))).unwrap();
    assert_eq!(ranges(&lib), [(0, 2500, Some(-1)), (0, 2500, Some(4500)), (0, 2500, Some(6500))]);
}
