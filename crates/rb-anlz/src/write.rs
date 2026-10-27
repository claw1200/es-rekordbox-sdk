//! Changing analysis files: swapping tags in place and building the path tag.

use crate::build::DETAIL_RATE;
use crate::decode::{self, Tags, TAG_HEADER};

/// Detail waveforms: one entry per column, at the rate the tag's header names.
const DETAIL_WAVES: [&str; 3] = ["PWV3", "PWV5", "PWV7"];

/// The `PPTH` tag holding the audio file's path as the analysis files record it.
#[must_use]
pub fn path_tag(name: &str) -> Vec<u8> {
    let mut body: Vec<u8> = name.encode_utf16().flat_map(u16::to_be_bytes).collect();
    body.extend([0, 0]);
    let header_len = u32::try_from(TAG_HEADER + 4).unwrap_or(0);
    let total = header_len + u32::try_from(body.len()).unwrap_or(0);
    let mut out = b"PPTH".to_vec();
    out.extend(header_len.to_be_bytes());
    out.extend(total.to_be_bytes());
    out.extend(u32::try_from(body.len()).unwrap_or(0).to_be_bytes());
    out.extend(body);
    out
}

/// `data` with the tags of the given kinds swapped in place, or added at the end when the file has none.
/// Every other tag is kept byte for byte.
pub fn replace_tags(data: &[u8], replace: &[(&str, Vec<u8>)]) -> std::io::Result<Vec<u8>> {
    patch_tags(data, replace, &[])
}

/// Like [`replace_tags`], and the tags of the `drop` kinds are removed.
pub fn patch_tags(data: &[u8], replace: &[(&str, Vec<u8>)], drop: &[&str]) -> std::io::Result<Vec<u8>> {
    let bad = |why: &str| std::io::Error::new(std::io::ErrorKind::InvalidData, why.to_owned());
    if data.len() < 12 || &data[..4] != b"PMAI" {
        return Err(bad("not an analysis file"));
    }
    let header = u32::from_be_bytes([data[4], data[5], data[6], data[7]]) as usize;
    if header < 12 {
        return Err(bad("header length is less than 12"));
    }
    let mut out = data.get(..header).ok_or_else(|| bad("short header"))?.to_vec();
    let mut pending: Vec<(&str, &Vec<u8>)> = replace.iter().map(|(k, v)| (*k, v)).collect();
    let mut pos = header;
    while pos + TAG_HEADER <= data.len() {
        let len = u32::from_be_bytes([data[pos + 8], data[pos + 9], data[pos + 10], data[pos + 11]]) as usize;
        if len < TAG_HEADER || pos + len > data.len() {
            break;
        }
        let kind = &data[pos..pos + 4];
        if let Some(at) = pending.iter().position(|(k, _)| k.as_bytes() == kind) {
            out.extend(pending.remove(at).1);
        } else if !replace.iter().any(|(k, _)| k.as_bytes() == kind) && !drop.iter().any(|k| k.as_bytes() == kind) {
            out.extend(&data[pos..pos + len]);
        }
        pos += len;
    }
    for (_, tag) in pending {
        out.extend(tag);
    }
    let total = u32::try_from(out.len()).map_err(|_| bad("file too large"))?;
    out[8..12].copy_from_slice(&total.to_be_bytes());
    Ok(out)
}

/// A `PQTZ` tag with the given beats, keeping the header of `tag`.
#[must_use]
pub fn build_beatgrid(tag: &[u8], times: &[u32], beats: &[u16], bpms: &[f64]) -> Vec<u8> {
    let len_header = u32::from_be_bytes([tag[4], tag[5], tag[6], tag[7]]) as usize;
    let mut header = tag[..len_header].to_vec();
    let body: Vec<u8> = times
        .iter()
        .zip(beats)
        .zip(bpms)
        .flat_map(|((time, beat), bpm)| {
            let tempo = (bpm * 100.0).round_ties_even() as u16;
            [beat.to_be_bytes().as_slice(), &tempo.to_be_bytes(), &time.to_be_bytes()].concat()
        })
        .collect();
    let total = u32::try_from(len_header + body.len()).unwrap_or(0);
    header[8..12].copy_from_slice(&total.to_be_bytes());
    header[20..24].copy_from_slice(&u32::try_from(times.len()).unwrap_or(0).to_be_bytes());
    header.extend(body);
    header
}

/// An empty `PQT2`, as rekordbox writes for many tracks. Its per-beat data can't be rebuilt, so players fall back to `PQTZ`.
#[must_use]
pub fn clear_extended_beatgrid(tag: &[u8]) -> Vec<u8> {
    let len_header = u32::from_be_bytes([tag[4], tag[5], tag[6], tag[7]]) as usize;
    let mut header = tag[..len_header].to_vec();
    header[24..].fill(0);
    header[8..12].copy_from_slice(&u32::try_from(len_header).unwrap_or(0).to_be_bytes());
    header
}

/// The tags of one analysis file rebuilt for audio whose timeline moved by `offset_ms`: beats move by the
/// offset (any that would land before 0 are dropped) and detail waveforms by whole columns, silent ones
/// filling the start or the earliest dropped. The extended grid is cleared. Preview waveforms span the
/// whole track and are kept: a delay is far less than one of their columns.
#[must_use]
pub fn timeline_patches(tags: &Tags, offset_ms: i64) -> Vec<(&'static str, Vec<u8>)> {
    if offset_ms == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    if let (Some(grid), Some(pqtz)) = (decode::beatgrid(tags), tags.first("PQTZ")) {
        let keep: Vec<usize> = (0..grid.times.len()).filter(|i| i64::from(grid.times[*i]) + offset_ms >= 0).collect();
        let times: Vec<u32> = keep.iter().map(|i| u32::try_from(i64::from(grid.times[*i]) + offset_ms).unwrap_or(u32::MAX)).collect();
        let beats: Vec<u16> = keep.iter().map(|i| grid.beats[*i]).collect();
        let bpms: Vec<f64> = keep.iter().map(|i| grid.bpms[*i]).collect();
        out.push(("PQTZ", build_beatgrid(pqtz, &times, &beats, &bpms)));
    }
    let header_fits = |tag: &[u8]| tag.get(4..8).is_some_and(|b| (24..=tag.len()).contains(&(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)));
    if let Some(pqt2) = tags.first("PQT2").filter(|tag| header_fits(tag)) {
        out.push(("PQT2", clear_extended_beatgrid(pqt2)));
    }
    for kind in DETAIL_WAVES {
        if let Some(shifted) = tags.first(kind).and_then(|tag| shift_columns(tag, offset_ms)) {
            out.push((kind, shifted));
        }
    }
    out
}

/// `data`, an analysis file, with [`timeline_patches`] applied.
pub fn shift_timeline(data: &[u8], offset_ms: i64) -> std::io::Result<Vec<u8>> {
    if offset_ms == 0 {
        return Ok(data.to_vec());
    }
    let patches = timeline_patches(&Tags::parse(data.to_vec())?, offset_ms);
    patch_tags(data, &patches, &[])
}

/// A detail waveform tag moved by `offset_ms`, rounded to whole columns. `None` when it doesn't move or
/// its header can't be read.
#[allow(clippy::cast_possible_truncation, clippy::cast_precision_loss, reason = "a delay of a few seconds at most")]
fn shift_columns(tag: &[u8], offset_ms: i64) -> Option<Vec<u8>> {
    let word = |at: usize| tag.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
    let len_header = word(4)?;
    let width = word(12)?;
    if len_header < 24 || len_header > tag.len() || width == 0 {
        return None;
    }
    let rate = match word(20)? >> 16 {
        0 => DETAIL_RATE as usize,
        rate => rate,
    };
    let columns = (offset_ms as f64 * rate as f64 / 1000.0).round() as i64;
    if columns == 0 {
        return None;
    }
    let body = &tag[len_header..];
    let body = &body[..word(16)?.min(body.len() / width) * width];
    let moved = usize::try_from(columns.unsigned_abs()).ok()? * width;
    let mut out = tag[..len_header].to_vec();
    if columns > 0 {
        out.resize(len_header + moved, 0);
        out.extend(body);
    } else {
        out.extend(&body[moved.min(body.len())..]);
    }
    let total = u32::try_from(out.len()).ok()?;
    let count = u32::try_from((out.len() - len_header) / width).ok()?;
    out[8..12].copy_from_slice(&total.to_be_bytes());
    out[16..20].copy_from_slice(&count.to_be_bytes());
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::Tags;

    fn file(tags: &[Vec<u8>]) -> Vec<u8> {
        let mut data = b"PMAI".to_vec();
        data.extend(28u32.to_be_bytes());
        data.extend([0u8; 20]);
        tags.iter().for_each(|t| data.extend(t));
        data
    }

    fn other(kind: &str) -> Vec<u8> {
        let mut t = kind.as_bytes().to_vec();
        t.extend(12u32.to_be_bytes());
        t.extend(16u32.to_be_bytes());
        t.extend([1, 2, 3, 4]);
        t
    }

    #[test]
    fn swaps_the_path_tag_and_keeps_the_rest() {
        let data = file(&[path_tag("?/old.wav"), other("PQTZ")]);
        let patched = replace_tags(&data, &[("PPTH", path_tag("?/new name.wav"))]).unwrap();
        let tags = Tags::parse(patched.clone()).unwrap();
        let ppth = tags.first("PPTH").unwrap();
        let text: Vec<u16> = ppth[16..ppth.len() - 2].chunks(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        assert_eq!(String::from_utf16(&text).unwrap(), "?/new name.wav");
        assert_eq!(tags.first("PQTZ").unwrap(), other("PQTZ").as_slice());
        assert_eq!(u32::from_be_bytes([patched[8], patched[9], patched[10], patched[11]]) as usize, patched.len());
    }

    #[test]
    fn adds_a_tag_the_file_lacks() {
        let patched = replace_tags(&file(&[other("PQTZ")]), &[("PPTH", path_tag("?/a.wav"))]).unwrap();
        assert!(Tags::parse(patched).unwrap().first("PPTH").is_some());
    }

    #[test]
    fn rejects_invalid_pmai_header_lengths() {
        for header in (0u32..12).chain([13, u32::MAX]) {
            let mut data = b"PMAI".to_vec();
            data.extend(header.to_be_bytes());
            data.extend(12u32.to_be_bytes());
            let error = patch_tags(&data, &[], &[]).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "header length {header}");
        }
    }

    #[test]
    fn rejects_invalid_pmai_headers_with_replacement_tags() {
        for header in 0u32..12 {
            let mut data = b"PMAI".to_vec();
            data.extend(header.to_be_bytes());
            data.extend(12u32.to_be_bytes());
            let error = replace_tags(&data, &[("PPTH", path_tag("?/a.wav"))]).unwrap_err();
            assert_eq!(error.kind(), std::io::ErrorKind::InvalidData, "header length {header}");
        }
    }

    #[test]
    fn accepts_a_twelve_byte_pmai_header() {
        let mut data = b"PMAI".to_vec();
        data.extend(12u32.to_be_bytes());
        data.extend(12u32.to_be_bytes());
        assert_eq!(patch_tags(&data, &[], &[]).unwrap(), data);

        let replacement = path_tag("?/a.wav");
        let patched = replace_tags(&data, &[("PPTH", replacement.clone())]).unwrap();
        assert_eq!(&patched[12..], replacement.as_slice());
        assert_eq!(u32::from_be_bytes(patched[8..12].try_into().unwrap()) as usize, patched.len());
    }

    #[test]
    fn refuses_other_files() {
        assert!(replace_tags(b"nope", &[]).is_err());
    }

    fn detail(kind: &str, width: u32, body: &[u8]) -> Vec<u8> {
        let mut t = kind.as_bytes().to_vec();
        t.extend(24u32.to_be_bytes());
        t.extend(u32::try_from(24 + body.len()).unwrap().to_be_bytes());
        t.extend(width.to_be_bytes());
        t.extend(u32::try_from(body.len()).unwrap().checked_div(width).unwrap().to_be_bytes());
        t.extend((150u32 << 16).to_be_bytes());
        t.extend(body);
        t
    }

    fn entries(tags: &Tags, kind: &str) -> (u32, Vec<u8>) {
        let tag = tags.first(kind).unwrap();
        (u32::from_be_bytes(tag[16..20].try_into().unwrap()), tag[24..].to_vec())
    }

    fn shifted(offset_ms: i64) -> Tags {
        let grid = crate::build::beatgrid_tag(&[(1, 12800, 10), (2, 12800, 479), (3, 12800, 948)]);
        let data = file(&[
            grid,
            detail("PWV3", 1, &[1, 2, 3, 4, 5]),
            detail("PWV5", 2, &[1, 1, 2, 2, 3, 3]),
            detail("PWV7", 3, &[1, 1, 1, 2, 2, 2]),
            detail("PWV4", 6, &[9; 12]),
        ]);
        Tags::parse(shift_timeline(&data, offset_ms).unwrap()).unwrap()
    }

    #[test]
    fn a_later_timeline_moves_beats_and_pads_detail_waveforms() {
        let tags = shifted(23);
        let grid = decode::beatgrid(&tags).unwrap();
        assert_eq!(grid.times, [33, 502, 971]);
        assert_eq!(grid.beats, [1, 2, 3]);
        // 23 ms is 3.45 columns at 150 a second.
        assert_eq!(entries(&tags, "PWV3"), (8, vec![0, 0, 0, 1, 2, 3, 4, 5]));
        assert_eq!(entries(&tags, "PWV5"), (6, [vec![0; 6], vec![1, 1, 2, 2, 3, 3]].concat()));
        assert_eq!(entries(&tags, "PWV7"), (5, [vec![0; 9], vec![1, 1, 1, 2, 2, 2]].concat()));
        assert_eq!(tags.first("PWV4").unwrap(), detail("PWV4", 6, &[9; 12]).as_slice());
    }

    #[test]
    fn an_earlier_timeline_drops_what_falls_before_the_start() {
        let tags = shifted(-20);
        let grid = decode::beatgrid(&tags).unwrap();
        assert_eq!(grid.times, [459, 928]);
        assert_eq!(grid.beats, [2, 3]);
        assert_eq!(entries(&tags, "PWV3"), (2, vec![4, 5]));
        assert_eq!(entries(&tags, "PWV5"), (0, Vec::new()));
        assert_eq!(entries(&tags, "PWV7"), (0, Vec::new()));
    }

    #[test]
    fn a_grid_moved_wholly_before_the_start_is_emptied_and_a_malformed_extended_grid_is_left() {
        let tags = shifted(-1000);
        assert_eq!(decode::beatgrid(&tags).map_or(0, |grid| grid.times.len()), 0);
        let data = file(&[other("PQT2")]);
        let kept = Tags::parse(shift_timeline(&data, 23).unwrap()).unwrap();
        assert_eq!(kept.first("PQT2").unwrap(), other("PQT2").as_slice());
    }

    #[test]
    fn no_offset_changes_nothing() {
        let data = file(&[detail("PWV3", 1, &[1, 2]), other("PQT2")]);
        assert_eq!(shift_timeline(&data, 0).unwrap(), data);
        assert!(timeline_patches(&Tags::parse(data).unwrap(), 0).is_empty());
        // Under half a column the waveforms stay put.
        let tags = Tags::parse(file(&[detail("PWV3", 1, &[1, 2])])).unwrap();
        assert!(timeline_patches(&tags, 3).is_empty());
    }

    #[test]
    fn the_empty_seek_table_is_rekordbox_sized() {
        let vbr = crate::build::empty_vbr_tag();
        assert_eq!(&vbr[..4], b"PVBR");
        assert_eq!(u32::from_be_bytes(vbr[8..12].try_into().unwrap()), 1620);
        assert!(vbr[12..].iter().all(|b| *b == 0));
    }

    #[test]
    fn path_edits_and_backend_roundtrip_preserve_unknown_sections() {
        let unknown = other("ZZZZ");
        let original = file(&[path_tag("?/old.wav"), unknown.clone()]);
        let patched = replace_tags(&original, &[("PPTH", path_tag("?/new.wav"))]).unwrap();
        let parsed = rbl_anlz::parse(&patched).unwrap();
        assert_eq!(parsed.path().as_deref(), Some("?/new.wav"));
        assert_eq!(parsed.to_bytes(), patched);
        let local = Tags::parse(parsed.to_bytes()).unwrap();
        assert_eq!(local.first("ZZZZ").unwrap(), unknown);
    }
}
