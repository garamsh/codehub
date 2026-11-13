//! The 3-way merge of one path's version: its kind and its content (ADR 0017, ADR 0019).

use std::fmt;
use std::ops::Range;

use imara_diff::sources::byte_lines;
use imara_diff::{Algorithm, Diff, InternedInput};

use crate::domain::file::FileKind;

/// How many leading bytes are searched for a NUL byte when telling binary from text.
const BINARY_SCAN_LEN: usize = 8000;

/// What one path holds: its kind and its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileVersion {
    /// Whether the file is executable.
    pub kind: FileKind,
    /// The file's bytes.
    pub content: Vec<u8>,
}

/// Why one path's versions do not merge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileConflict {
    /// One side deleted the path and the other modified it.
    DeleteModify,
    /// Both sides added the path with different content.
    AddAdd,
    /// Both sides changed the same lines of a text file differently.
    Lines,
    /// Both sides changed the path to different content, and one of its versions is binary.
    Binary,
    /// Both sides set the path's kind differently.
    Kind,
}

impl fmt::Display for FileConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DeleteModify => "deleted on one side and modified on the other",
            Self::AddAdd => "added on both sides with different content",
            Self::Lines => "the same lines changed differently on both sides",
            Self::Binary => "a binary file changed differently on both sides",
            Self::Kind => "the file kind set differently on both sides",
        })
    }
}

impl std::error::Error for FileConflict {}

/// Merges one path's version at the base with its version on two sides.
///
/// `None` is the path's absence. Returns the merged version, `None` where the
/// merge deletes the path. The kind and the content are merged apart, so one
/// side's change to the kind is kept beside the other side's change to the
/// content. Content is merged by lines only when its base and both sides are
/// text; otherwise a change on both sides to different content conflicts.
///
/// # Errors
///
/// Returns the [`FileConflict`] that stops the merge; where the content and
/// the kind both conflict, the content's.
pub fn merge_file(
    base: Option<&FileVersion>,
    ours: Option<&FileVersion>,
    theirs: Option<&FileVersion>,
) -> Result<Option<FileVersion>, FileConflict> {
    if ours == theirs || theirs == base {
        return Ok(ours.cloned());
    }
    if ours == base {
        return Ok(theirs.cloned());
    }
    match (ours, theirs) {
        (Some(ours), Some(theirs)) => {
            let content = merge_content(
                base.map(|base| base.content.as_slice()),
                &ours.content,
                &theirs.content,
            )?;
            let kind = merge_kind(base.map(|base| base.kind), ours.kind, theirs.kind)?;
            Ok(Some(FileVersion { kind, content }))
        }
        (None, _) | (_, None) => Err(FileConflict::DeleteModify),
    }
}

fn merge_content(base: Option<&[u8]>, ours: &[u8], theirs: &[u8]) -> Result<Vec<u8>, FileConflict> {
    if ours == theirs || Some(theirs) == base {
        return Ok(ours.to_vec());
    }
    if Some(ours) == base {
        return Ok(theirs.to_vec());
    }
    let Some(base) = base else {
        return Err(FileConflict::AddAdd);
    };
    if is_text(base) && is_text(ours) && is_text(theirs) {
        merge_lines(base, ours, theirs).ok_or(FileConflict::Lines)
    } else {
        Err(FileConflict::Binary)
    }
}

fn merge_kind(
    base: Option<FileKind>,
    ours: FileKind,
    theirs: FileKind,
) -> Result<FileKind, FileConflict> {
    if ours == theirs || Some(theirs) == base {
        Ok(ours)
    } else if Some(ours) == base {
        Ok(theirs)
    } else {
        Err(FileConflict::Kind)
    }
}

/// Whether `content` is text: no NUL in its first bytes and valid UTF-8 (ADR 0017).
pub(crate) fn is_text(content: &[u8]) -> bool {
    !content.iter().take(BINARY_SCAN_LEN).any(|byte| *byte == 0)
        && std::str::from_utf8(content).is_ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    Ours,
    Theirs,
}

/// One side's replacement of the base lines `before` by its lines `after`.
struct Edit {
    side: Side,
    before: Range<usize>,
    after: Range<usize>,
}

/// Merges by lines; `None` where both sides change the same lines differently.
fn merge_lines(base: &[u8], ours: &[u8], theirs: &[u8]) -> Option<Vec<u8>> {
    let base_lines: Vec<&[u8]> = byte_lines(base).collect();
    let ours_lines: Vec<&[u8]> = byte_lines(ours).collect();
    let theirs_lines: Vec<&[u8]> = byte_lines(theirs).collect();

    let mut edits = diff_edits(base, ours, Side::Ours);
    edits.extend(diff_edits(base, theirs, Side::Theirs));
    // An insertion sorts before a change starting at the same base line, so regions come out in base order.
    edits.sort_by_key(|edit| (edit.before.start, edit.before.end));

    let mut merged = Vec::new();
    let mut copied_to = 0;
    let mut edits = edits.into_iter().peekable();
    while let Some(first) = edits.next() {
        let mut region = first.before.clone();
        let mut ours_edits = Vec::new();
        let mut theirs_edits = Vec::new();
        let mut next = Some(first);
        while let Some(edit) = next {
            region.end = region.end.max(edit.before.end);
            match edit.side {
                Side::Ours => ours_edits.push(edit),
                Side::Theirs => theirs_edits.push(edit),
            }
            next = edits.next_if(|edit| overlaps(&region, &edit.before));
        }

        extend_lines(&mut merged, &base_lines, copied_to..region.start);
        if theirs_edits.is_empty() {
            apply(&mut merged, &region, &ours_edits, &base_lines, &ours_lines);
        } else if ours_edits.is_empty() {
            apply(
                &mut merged,
                &region,
                &theirs_edits,
                &base_lines,
                &theirs_lines,
            );
        } else {
            let mut ours_text = Vec::new();
            apply(
                &mut ours_text,
                &region,
                &ours_edits,
                &base_lines,
                &ours_lines,
            );
            let mut theirs_text = Vec::new();
            apply(
                &mut theirs_text,
                &region,
                &theirs_edits,
                &base_lines,
                &theirs_lines,
            );
            if ours_text != theirs_text {
                return None;
            }
            merged.extend(ours_text);
        }
        copied_to = region.end;
    }
    extend_lines(&mut merged, &base_lines, copied_to..base_lines.len());
    Some(merged)
}

/// One side's edits of the base, by the Histogram line diff.
fn diff_edits(base: &[u8], side_content: &[u8], side: Side) -> Vec<Edit> {
    let input = InternedInput::new(base, side_content);
    Diff::compute(Algorithm::Histogram, &input)
        .hunks()
        .map(|hunk| Edit {
            side,
            before: widen(&hunk.before),
            after: widen(&hunk.after),
        })
        .collect()
}

fn widen(range: &Range<u32>) -> Range<usize> {
    range.start as usize..range.end as usize
}

/// Whether an edit of the base lines `next` touches the base lines `region` covers.
///
/// Ranges that only meet at an end do not overlap; two insertions overlap at
/// the same base position, and an insertion overlaps a change strictly around it.
fn overlaps(region: &Range<usize>, next: &Range<usize>) -> bool {
    match (region.is_empty(), next.is_empty()) {
        (true, true) => region.start == next.start,
        (true, false) => next.start < region.start && region.start < next.end,
        (false, true) => region.start < next.start && next.start < region.end,
        (false, false) => region.start < next.end && next.start < region.end,
    }
}

/// Writes the base lines of `region` with one side's `edits` applied.
fn apply(
    out: &mut Vec<u8>,
    region: &Range<usize>,
    edits: &[Edit],
    base_lines: &[&[u8]],
    side_lines: &[&[u8]],
) {
    let mut position = region.start;
    for edit in edits {
        extend_lines(out, base_lines, position..edit.before.start);
        extend_lines(out, side_lines, edit.after.clone());
        position = edit.before.end;
    }
    extend_lines(out, base_lines, position..region.end);
}

fn extend_lines(out: &mut Vec<u8>, lines: &[&[u8]], range: Range<usize>) {
    for line in lines.iter().skip(range.start).take(range.len()) {
        out.extend_from_slice(line);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn merge_text(base: &str, ours: &str, theirs: &str) -> Result<Vec<u8>, FileConflict> {
        merge_content(Some(base.as_bytes()), ours.as_bytes(), theirs.as_bytes())
    }

    fn clean(text: &str) -> Result<Vec<u8>, FileConflict> {
        Ok(text.as_bytes().to_vec())
    }

    fn file(content: &str) -> Option<FileVersion> {
        Some(FileVersion {
            kind: FileKind::Regular,
            content: content.as_bytes().to_vec(),
        })
    }

    fn executable(content: &str) -> Option<FileVersion> {
        Some(FileVersion {
            kind: FileKind::Executable,
            content: content.as_bytes().to_vec(),
        })
    }

    #[test]
    fn keeps_changes_to_different_lines_from_both_sides() {
        assert_eq!(
            merge_text("a\nb\nc\nd\ne\n", "A\nb\nc\nd\ne\n", "a\nb\nc\nd\nE\n"),
            clean("A\nb\nc\nd\nE\n")
        );
    }

    #[test]
    fn merges_the_same_lines_changed_identically() {
        assert_eq!(
            merge_text("a\nb\nc\n", "a\nB\nc\nd\n", "a\nB\nc\nd\n"),
            clean("a\nB\nc\nd\n")
        );
    }

    #[test]
    fn merges_an_identical_change_beside_a_one_sided_change() {
        assert_eq!(
            merge_text("a\nb\nc\nd\n", "A\nb\nc\nD\n", "A\nb\nc\nd\n"),
            clean("A\nb\nc\nD\n")
        );
    }

    #[test]
    fn conflicts_on_the_same_lines_changed_differently() {
        assert_eq!(
            merge_text("a\nb\nc\n", "a\nX\nc\n", "a\nY\nc\n"),
            Err(FileConflict::Lines)
        );
        // Control: the same two edits on lines apart merge.
        assert_eq!(
            merge_text("a\nb\nc\n", "a\nX\nc\n", "a\nb\nY\n"),
            clean("a\nX\nY\n")
        );
    }

    #[test]
    fn conflicts_on_partly_overlapping_line_ranges() {
        assert_eq!(
            merge_text("a\nb\nc\nd\n", "a\nX\nX\nd\n", "a\nb\nY\nY\n"),
            Err(FileConflict::Lines)
        );
    }

    #[test]
    fn merges_changes_to_adjacent_line_ranges() {
        assert_eq!(
            merge_text("a\nb\nc\nd\n", "a\nB\nc\nd\n", "a\nb\nC\nd\n"),
            clean("a\nB\nC\nd\n")
        );
    }

    #[test]
    fn conflicts_on_different_insertions_at_the_same_point() {
        assert_eq!(
            merge_text("a\nb\n", "a\nX\nb\n", "a\nY\nb\n"),
            Err(FileConflict::Lines)
        );
        // Control: the same insertions at different points merge.
        assert_eq!(
            merge_text("a\nb\n", "X\na\nb\n", "a\nY\nb\n"),
            clean("X\na\nY\nb\n")
        );
    }

    #[test]
    fn merges_identical_insertions_at_the_same_point() {
        assert_eq!(
            merge_text("a\nb\n", "a\nX\nb\n", "a\nX\nb\n"),
            clean("a\nX\nb\n")
        );
    }

    #[test]
    fn conflicts_on_an_insertion_inside_lines_the_other_side_changed() {
        assert_eq!(
            merge_text("a\nb\nc\nd\n", "a\nb\nX\nc\nd\n", "a\nB\nC\nd\n"),
            Err(FileConflict::Lines)
        );
    }

    #[test]
    fn merges_an_insertion_at_the_edge_of_lines_the_other_side_changed() {
        assert_eq!(
            merge_text("a\nb\nc\nd\n", "a\nb\nc\nX\nd\n", "a\nB\nC\nd\n"),
            clean("a\nB\nC\nX\nd\n")
        );
    }

    #[test]
    fn counts_a_whitespace_only_change_as_a_change() {
        assert_eq!(
            merge_text("a\nb\nc\n", "a\nb \nc\n", "a\nB\nc\n"),
            Err(FileConflict::Lines)
        );
    }

    #[test]
    fn counts_a_line_ending_change_as_a_change() {
        assert_eq!(
            merge_text("a\nb\nc\n", "a\nb\r\nc\n", "a\nB\nc\n"),
            Err(FileConflict::Lines)
        );
    }

    #[test]
    fn keeps_a_missing_final_newline_from_the_side_that_removed_it() {
        assert_eq!(
            merge_text("a\nb\nc\n", "A\nb\nc\n", "a\nb\nc"),
            clean("A\nb\nc")
        );
    }

    #[test]
    fn conflicts_on_a_binary_path_changed_differently() {
        assert_eq!(
            merge_content(Some(b"a\0b\nc\n"), b"X\0b\nc\n", b"a\0b\nY\n"),
            Err(FileConflict::Binary)
        );
    }

    #[test]
    fn takes_the_changed_side_of_a_binary_path_changed_once() {
        assert_eq!(
            merge_content(Some(b"a\0"), b"a\0", b"b\0"),
            Ok(b"b\0".to_vec())
        );
    }

    #[test]
    fn treats_a_nul_byte_in_the_first_8000_bytes_as_binary() {
        let mut base = vec![b'x'; 7999];
        base.extend_from_slice(b"\0\na\nb\n");
        let ours = [base.as_slice(), b"c\n"].concat();
        let theirs = [b"c\n".as_slice(), base.as_slice()].concat();
        assert_eq!(
            merge_content(Some(&base), &ours, &theirs),
            Err(FileConflict::Binary)
        );
    }

    #[test]
    fn treats_a_nul_byte_after_the_first_8000_bytes_as_text() {
        let mut base = vec![b'x'; 8000];
        base.extend_from_slice(b"\0\na\nb\n");
        let ours = [base.as_slice(), b"c\n"].concat();
        let theirs = [b"c\n".as_slice(), base.as_slice()].concat();
        let merged = [b"c\n".as_slice(), base.as_slice(), b"c\n"].concat();
        assert_eq!(merge_content(Some(&base), &ours, &theirs), Ok(merged));
    }

    #[test]
    fn treats_invalid_utf8_as_binary() {
        assert_eq!(
            merge_content(Some(b"a\n\xff\nb\n"), b"A\n\xff\nb\n", b"a\n\xff\nB\n"),
            Err(FileConflict::Binary)
        );
        // Control: the same edits around a valid line merge.
        assert_eq!(
            merge_text("a\nz\nb\n", "A\nz\nb\n", "a\nz\nB\n"),
            clean("A\nz\nB\n")
        );
    }

    #[test]
    fn conflicts_on_a_deletion_against_a_modification() {
        assert_eq!(
            merge_file(file("a\n").as_ref(), None, file("b\n").as_ref()),
            Err(FileConflict::DeleteModify)
        );
        assert_eq!(
            merge_file(file("a\n").as_ref(), file("b\n").as_ref(), None),
            Err(FileConflict::DeleteModify)
        );
    }

    #[test]
    fn conflicts_on_a_deletion_against_a_kind_change() {
        assert_eq!(
            merge_file(file("a\n").as_ref(), None, executable("a\n").as_ref()),
            Err(FileConflict::DeleteModify)
        );
        // Control: the same deletion against an unchanged path deletes it.
        assert_eq!(
            merge_file(file("a\n").as_ref(), None, file("a\n").as_ref()),
            Ok(None)
        );
    }

    #[test]
    fn deletes_a_path_deleted_on_one_side_and_unchanged_on_the_other() {
        assert_eq!(
            merge_file(file("a\n").as_ref(), file("a\n").as_ref(), None),
            Ok(None)
        );
    }

    #[test]
    fn deletes_a_path_deleted_on_both_sides() {
        assert_eq!(merge_file(file("a\n").as_ref(), None, None), Ok(None));
    }

    #[test]
    fn conflicts_on_a_path_added_on_both_sides_with_different_content() {
        assert_eq!(
            merge_file(None, file("a\n").as_ref(), file("b\n").as_ref()),
            Err(FileConflict::AddAdd)
        );
    }

    #[test]
    fn merges_a_path_added_on_both_sides_with_identical_content() {
        assert_eq!(
            merge_file(None, file("a\n").as_ref(), file("a\n").as_ref()),
            Ok(file("a\n"))
        );
    }

    #[test]
    fn takes_a_kind_changed_on_one_side_beside_a_content_change_on_the_other() {
        assert_eq!(
            merge_file(
                file("a\n").as_ref(),
                executable("a\n").as_ref(),
                file("b\n").as_ref()
            ),
            Ok(executable("b\n"))
        );
        assert_eq!(
            merge_file(
                file("a\n").as_ref(),
                file("b\n").as_ref(),
                executable("a\n").as_ref()
            ),
            Ok(executable("b\n"))
        );
    }

    #[test]
    fn takes_a_kind_changed_identically_on_both_sides() {
        assert_eq!(
            merge_file(
                file("a\nb\n").as_ref(),
                executable("A\nb\n").as_ref(),
                executable("a\nB\n").as_ref()
            ),
            Ok(executable("A\nB\n"))
        );
    }

    #[test]
    fn conflicts_on_a_path_added_on_both_sides_as_different_kinds() {
        assert_eq!(
            merge_file(None, file("a\n").as_ref(), executable("a\n").as_ref()),
            Err(FileConflict::Kind)
        );
        // Control: the same addition as one kind on both sides merges.
        assert_eq!(
            merge_file(None, executable("a\n").as_ref(), executable("a\n").as_ref()),
            Ok(executable("a\n"))
        );
    }

    #[test]
    fn reports_the_content_conflict_where_content_and_kind_both_conflict() {
        assert_eq!(
            merge_file(None, file("a\n").as_ref(), executable("b\n").as_ref()),
            Err(FileConflict::AddAdd)
        );
    }
}
