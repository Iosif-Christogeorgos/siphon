//! The shape of an MKV's chapters, read from the file itself. ffprobe
//! flattens them to one plain list, and FFmpeg writes them so: whether a
//! file has more (another edition, chapters within chapters, hidden ones,
//! names in several languages, an edition that plays its parts in an order
//! of its own) is known only here, and said before a copy flattens them.
//!
//! Only the Chapters element is read: found among the segment's first
//! elements, where muxers put it, or through its seek index.

use std::io::{Read, Seek, SeekFrom};

use serde::Serialize;

const EBML: u32 = 0x1A45_DFA3;
const SEGMENT: u32 = 0x1853_8067;
const SEEK_HEAD: u32 = 0x114D_9B74;
const SEEK: u32 = 0x4DBB;
const SEEK_ID: u32 = 0x53AB;
const SEEK_POSITION: u32 = 0x53AC;
const CHAPTERS: u32 = 0x1043_A770;
const CLUSTER: u32 = 0x1F43_B675;
const EDITION: u32 = 0x45B9;
const EDITION_ORDERED: u32 = 0x45DD;
const ATOM: u32 = 0xB6;
const ATOM_HIDDEN: u32 = 0x98;
const ATOM_DISPLAY: u32 = 0x80;
const ATOM_SEGMENT: u32 = 0x6E67;

/// Chapters larger than this aren't read: a film's are a few kilobytes.
const LARGEST: u64 = 4 * 1024 * 1024;
/// The segment's elements looked through for the chapters before its first
/// cluster, at most; then its seek index is asked.
const LOOKED_THROUGH: usize = 64;
/// Chapters within chapters deeper than this aren't followed.
const DEEPEST: usize = 16;

/// What a file's chapters are, beyond the plain list a copy keeps.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Shape {
    /// Its versions of the chapters (editions): a copy keeps the first.
    pub editions: usize,
    /// Chapters within chapters.
    pub nested: bool,
    /// Chapters a player doesn't list.
    pub hidden: bool,
    /// Chapters named more than once, in several languages.
    pub names: bool,
    /// An edition that plays the file's parts in an order of its own, or parts of other files.
    pub ordered: bool,
}

impl Shape {
    /// Nothing a copy would lose: one list, one name a chapter.
    pub fn plain(&self) -> bool {
        self.editions <= 1 && !self.nested && !self.hidden && !self.names && !self.ordered
    }
}

/// The shape of the chapters of the MKV `file`; None if it has none, or
/// isn't an MKV that can be read.
pub fn shape(file: &mut (impl Read + Seek)) -> Option<Shape> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let (id, size) = header(file)?;
    if id != EBML {
        return None;
    }
    file.seek(SeekFrom::Current(i64::try_from(size?).ok()?)).ok()?;
    let (id, _) = header(file)?;
    if id != SEGMENT {
        return None;
    }
    let segment = file.stream_position().ok()?;
    let mut indexed = None;
    for _ in 0..LOOKED_THROUGH {
        let Some((id, size)) = header(file) else { break };
        match (id, size) {
            (CHAPTERS, Some(size)) => return read_body(file, size).map(|body| chapters(&body)),
            (SEEK_HEAD, Some(size)) => indexed = read_body(file, size).and_then(|body| chapters_at(&body)),
            (CLUSTER, _) | (_, None) => break,
            (_, Some(size)) => {
                file.seek(SeekFrom::Current(i64::try_from(size).ok()?)).ok()?;
            }
        }
    }
    file.seek(SeekFrom::Start(segment.checked_add(indexed?)?)).ok()?;
    match header(file)? {
        (CHAPTERS, Some(size)) => read_body(file, size).map(|body| chapters(&body)),
        _ => None,
    }
}

/// Where the seek index says the chapters are, from the segment's start.
fn chapters_at(seek_head: &[u8]) -> Option<u64> {
    elements(seek_head).filter(|(id, _)| *id == SEEK).find_map(|(_, seek)| {
        let mut id = None;
        let mut position = None;
        for (child, body) in elements(seek) {
            match child {
                SEEK_ID => id = Some(unsigned(body)),
                SEEK_POSITION => position = Some(unsigned(body)),
                _ => {}
            }
        }
        (id == Some(u64::from(CHAPTERS))).then_some(position).flatten()
    })
}

fn chapters(body: &[u8]) -> Shape {
    let mut shape = Shape::default();
    for (_, edition) in elements(body).filter(|(id, _)| *id == EDITION) {
        shape.editions += 1;
        for (id, child) in elements(edition) {
            match id {
                EDITION_ORDERED if unsigned(child) == 1 => shape.ordered = true,
                ATOM => atom(child, 0, &mut shape),
                _ => {}
            }
        }
    }
    shape
}

fn atom(body: &[u8], depth: usize, shape: &mut Shape) {
    let mut names = 0;
    for (id, child) in elements(body) {
        match id {
            ATOM => {
                shape.nested = true;
                if depth < DEEPEST {
                    atom(child, depth + 1, shape);
                }
            }
            ATOM_HIDDEN if unsigned(child) == 1 => shape.hidden = true,
            ATOM_DISPLAY => names += 1,
            ATOM_SEGMENT => shape.ordered = true,
            _ => {}
        }
    }
    shape.names |= names > 1;
}

/// An element's ID and size, from where `file` is; the size is None when
/// the element doesn't say it (a segment or cluster being written live).
fn header(file: &mut impl Read) -> Option<(u32, Option<u64>)> {
    let mut first = [0];
    file.read_exact(&mut first).ok()?;
    let length = first[0].leading_zeros() as usize + 1;
    if length > 4 {
        return None;
    }
    let mut id = u32::from(first[0]);
    for _ in 1..length {
        file.read_exact(&mut first).ok()?;
        id = id << 8 | u32::from(first[0]);
    }
    file.read_exact(&mut first).ok()?;
    let length = first[0].leading_zeros() as usize + 1;
    if length > 8 {
        return None;
    }
    let mut rest = [0; 7];
    file.read_exact(&mut rest[..length - 1]).ok()?;
    Some((id, size(first[0], &rest[..length - 1])))
}

fn read_body(file: &mut impl Read, size: u64) -> Option<Vec<u8>> {
    if size > LARGEST {
        return None;
    }
    let mut body = vec![0; usize::try_from(size).ok()?];
    file.read_exact(&mut body).ok()?;
    Some(body)
}

/// The elements one after another in `data`, up to the first that doesn't
/// fit in it.
fn elements(mut data: &[u8]) -> impl Iterator<Item = (u32, &[u8])> + '_ {
    std::iter::from_fn(move || {
        let mut reader = data;
        let (id, size) = header(&mut reader)?;
        let size = usize::try_from(size?).ok()?;
        let body = reader.get(..size)?;
        data = &reader[size..];
        Some((id, body))
    })
}

/// A size from its first byte and the rest; None when every bit of it is set,
/// which means unknown.
fn size(first: u8, rest: &[u8]) -> Option<u64> {
    let marker = 0x80u8 >> rest.len();
    let mut value = u64::from(first & (marker - 1));
    let mut unknown = value == u64::from(marker - 1);
    for &byte in rest {
        value = value << 8 | u64::from(byte);
        unknown &= byte == 0xFF;
    }
    (!unknown).then_some(value)
}

fn unsigned(body: &[u8]) -> u64 {
    body.iter().take(8).fold(0, |value, &byte| value << 8 | u64::from(byte))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    /// An element: its ID as written, and its size in eight bytes.
    fn element(id: u32, body: &[u8]) -> Vec<u8> {
        let mut out: Vec<u8> = id.to_be_bytes().into_iter().skip_while(|byte| *byte == 0).collect();
        out.push(0x01);
        out.extend_from_slice(&(body.len() as u64).to_be_bytes()[1..]);
        out.extend_from_slice(body);
        out
    }

    fn flag(id: u32) -> Vec<u8> {
        element(id, &[1])
    }

    fn chapter(names: usize, children: &[Vec<u8>]) -> Vec<u8> {
        let mut body = element(0x73C4, &[7]); // ChapterUID
        for _ in 0..names {
            body.extend(element(ATOM_DISPLAY, &element(0x85, b"Part")));
        }
        body.extend(children.concat());
        element(ATOM, &body)
    }

    /// An MKV: its header, then a segment of unknown size holding `elements`.
    fn mkv(elements: &[Vec<u8>]) -> Cursor<Vec<u8>> {
        let mut file = element(EBML, &element(0x4282, b"matroska"));
        file.extend([0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        file.extend(elements.concat());
        Cursor::new(file)
    }

    #[test]
    fn a_plain_list_of_chapters_is_plain() {
        let chapters = element(
            CHAPTERS,
            &element(EDITION, &[chapter(1, &[]), chapter(1, &[])].concat()),
        );
        let shape = shape(&mut mkv(&[element(0x1549_A966, b"info"), chapters])).unwrap();
        assert_eq!(
            shape,
            Shape {
                editions: 1,
                ..Shape::default()
            }
        );
        assert!(shape.plain());
    }

    #[test]
    fn finds_what_a_copy_would_flatten() {
        let first = element(
            EDITION,
            &[chapter(2, &[chapter(1, &[])]), chapter(1, &[flag(ATOM_HIDDEN)])].concat(),
        );
        let second = element(EDITION, &[flag(EDITION_ORDERED), chapter(1, &[])].concat());
        let shape = shape(&mut mkv(&[element(CHAPTERS, &[first, second].concat())])).unwrap();
        assert_eq!(
            shape,
            Shape {
                editions: 2,
                nested: true,
                hidden: true,
                names: true,
                ordered: true
            }
        );
        assert!(!shape.plain());
    }

    #[test]
    fn finds_chapters_after_the_clusters_through_the_seek_index() {
        let info = element(0x1549_A966, b"info");
        let cluster = element(CLUSTER, &[0; 64]);
        // The seek head itself, then the info, then the cluster: the chapters come after.
        let seek_head_len = element(
            SEEK_HEAD,
            &element(
                SEEK,
                &[element(SEEK_ID, &[0; 4]), element(SEEK_POSITION, &[0; 8])].concat(),
            ),
        )
        .len();
        let position = (seek_head_len + info.len() + cluster.len()) as u64;
        let seek = element(
            SEEK,
            &[
                element(SEEK_ID, &CHAPTERS.to_be_bytes()),
                element(SEEK_POSITION, &position.to_be_bytes()),
            ]
            .concat(),
        );
        let seek_head = element(SEEK_HEAD, &seek);
        assert_eq!(seek_head.len(), seek_head_len);
        let chapters = element(
            CHAPTERS,
            &[element(EDITION, &chapter(1, &[])), element(EDITION, &chapter(1, &[]))].concat(),
        );
        let shape = shape(&mut mkv(&[seek_head, info, cluster, chapters])).unwrap();
        assert_eq!(shape.editions, 2);
    }

    #[test]
    fn none_without_chapters_or_from_what_isnt_an_mkv() {
        assert_eq!(
            shape(&mut mkv(&[element(0x1549_A966, b"info"), element(CLUSTER, &[0; 8])])),
            None
        );
        assert_eq!(shape(&mut Cursor::new(b"RIFF....WAVEfmt ".to_vec())), None);
        assert_eq!(shape(&mut Cursor::new(Vec::new())), None);
        // Cut off in the middle of the chapters.
        let mut cut = mkv(&[element(CHAPTERS, &element(EDITION, &chapter(1, &[])))]).into_inner();
        cut.truncate(cut.len() - 5);
        assert_eq!(shape(&mut Cursor::new(cut)), None);
    }

    #[test]
    fn reads_sizes_and_unknown_ones() {
        assert_eq!(size(0x81, &[]), Some(1));
        assert_eq!(size(0x40, &[0x02]), Some(2));
        assert_eq!(size(0xFF, &[]), None);
        assert_eq!(size(0x01, &[0xFF; 7]), None);
        assert_eq!(size(0x01, &[0, 0, 0, 0, 0, 0x10, 0x00]), Some(4096));
    }
}
