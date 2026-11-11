//! The 3-way merge of one path's content (ADR 0017).

use std::fmt;
use std::ops::Range;

use imara_diff::sources::byte_lines;
use imara_diff::{Algorithm, Diff, InternedInput};

/// How many leading bytes are searched for a NUL byte when telling binary from text.
const BINARY_SCAN_LEN: usize = 8000;

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
}

impl fmt::Display for FileConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::DeleteModify => "deleted on one side and modified on the other",
            Self::AddAdd => "added on both sides with different content",
            Self::Lines => "the same lines changed differently on both sides",
            Self::Binary => "a binary file changed differently on both sides",
        })
    }
}

impl std::error::Error for FileConflict {}

/// Merges one path's content at the base with its content on two sides.
///
/// `None` is the path's absence. Returns the merged content, `None` where the
/// merge deletes the path. A path is merged by lines only when its base and
/// both sides are text; otherwise a change on both sides to different content
/// conflicts.
///
/// # Errors
///
/// Returns the [`FileConflict`] that stops the merge.
pub fn merge_file(
    base: Option<&[u8]>,
    ours: Option<&[u8]>,
    theirs: Option<&[u8]>,
) -> Result<Option<Vec<u8>>, FileConflict> {
    if ours == theirs || theirs == base {
        return Ok(ours.map(<[u8]>::to_vec));
    }
    if ours == base {
        return Ok(theirs.map(<[u8]>::to_vec));
    }
    match (base, ours, theirs) {
        (None, _, _) => Err(FileConflict::AddAdd),
        (Some(_), None, _) | (Some(_), _, None) => Err(FileConflict::DeleteModify),
        (Some(base), Some(ours), Some(theirs)) => {
            if is_text(base) && is_text(ours) && is_text(theirs) {
                merge_lines(base, ours, theirs)
                    .map(Some)
                    .ok_or(FileConflict::Lines)
            } else {
                Err(FileConflict::Binary)
            }
        }
    }
}

fn is_text(content: &[u8]) -> bool {
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

    fn merge_text(base: &str, ours: &str, theirs: &str) -> Result<Option<Vec<u8>>, FileConflict> {
        merge_file(
            Some(base.as_bytes()),
            Some(ours.as_bytes()),
            Some(theirs.as_bytes()),
        )
    }

    fn clean(text: &str) -> Result<Option<Vec<u8>>, FileConflict> {
        Ok(Some(text.as_bytes().to_vec()))
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
            merge_file(Some(b"a\0b\nc\n"), Some(b"X\0b\nc\n"), Some(b"a\0b\nY\n")),
            Err(FileConflict::Binary)
        );
    }

    #[test]
    fn takes_the_changed_side_of_a_binary_path_changed_once() {
        assert_eq!(
            merge_file(Some(b"a\0"), Some(b"a\0"), Some(b"b\0")),
            Ok(Some(b"b\0".to_vec()))
        );
    }

    #[test]
    fn treats_a_nul_byte_within_the_scanned_bytes_as_binary() {
        let mut base = vec![b'x'; BINARY_SCAN_LEN - 1];
        base.extend_from_slice(b"\0\na\nb\n");
        let ours = [base.as_slice(), b"c\n"].concat();
        let theirs = [b"c\n".as_slice(), base.as_slice()].concat();
        assert_eq!(
            merge_file(Some(&base), Some(&ours), Some(&theirs)),
            Err(FileConflict::Binary)
        );
    }

    #[test]
    fn treats_a_nul_byte_past_the_scanned_bytes_as_text() {
        let mut base = vec![b'x'; BINARY_SCAN_LEN];
        base.extend_from_slice(b"\0\na\nb\n");
        let ours = [base.as_slice(), b"c\n"].concat();
        let theirs = [b"c\n".as_slice(), base.as_slice()].concat();
        let merged = [b"c\n".as_slice(), base.as_slice(), b"c\n"].concat();
        assert_eq!(
            merge_file(Some(&base), Some(&ours), Some(&theirs)),
            Ok(Some(merged))
        );
    }

    #[test]
    fn treats_invalid_utf8_as_binary() {
        assert_eq!(
            merge_file(
                Some(b"a\n\xff\nb\n"),
                Some(b"A\n\xff\nb\n"),
                Some(b"a\n\xff\nB\n")
            ),
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
            merge_file(Some(b"a\n"), None, Some(b"b\n")),
            Err(FileConflict::DeleteModify)
        );
        assert_eq!(
            merge_file(Some(b"a\n"), Some(b"b\n"), None),
            Err(FileConflict::DeleteModify)
        );
    }

    #[test]
    fn deletes_a_path_deleted_on_one_side_and_unchanged_on_the_other() {
        assert_eq!(merge_file(Some(b"a\n"), Some(b"a\n"), None), Ok(None));
    }

    #[test]
    fn deletes_a_path_deleted_on_both_sides() {
        assert_eq!(merge_file(Some(b"a\n"), None, None), Ok(None));
    }

    #[test]
    fn conflicts_on_a_path_added_on_both_sides_with_different_content() {
        assert_eq!(
            merge_file(None, Some(b"a\n"), Some(b"b\n")),
            Err(FileConflict::AddAdd)
        );
    }

    #[test]
    fn merges_a_path_added_on_both_sides_with_identical_content() {
        assert_eq!(
            merge_file(None, Some(b"a\n"), Some(b"a\n")),
            Ok(Some(b"a\n".to_vec()))
        );
    }
}
