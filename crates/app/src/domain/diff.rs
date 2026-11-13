//! The difference between a path's content in the base and in a draft (ADR 0048): a unified line
//! diff where both sides are text (ADR 0017), a marker otherwise.

use std::ops::Range;

use imara_diff::sources::byte_lines;
use imara_diff::{Algorithm, Diff, InternedInput};

use super::file::FileKind;
use super::merge::is_text;
use super::path::Path;

/// Lines of unchanged context around each hunk.
const CONTEXT: usize = 3;

/// How a path's content differs between two sides.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ContentDiff {
    /// Both sides are text: the unified diff.
    Lines(String),
    /// A side is not text, and the content differs.
    Binary,
    /// Both sides hold the same content and differ only in kind.
    KindOnly,
}

/// A side of a diff: the file's kind and its content.
pub type Side<'a> = Option<(FileKind, &'a [u8])>;

/// The difference of `path` from `before` to `after`; an absent side counts as empty text.
#[must_use]
pub fn diff(path: &Path, before: Side<'_>, after: Side<'_>) -> ContentDiff {
    let before_content = before.map_or(&[][..], |(_, content)| content);
    let after_content = after.map_or(&[][..], |(_, content)| content);
    if before.is_some() && after.is_some() && before_content == after_content {
        return ContentDiff::KindOnly;
    }
    if !is_text(before_content) || !is_text(after_content) {
        return ContentDiff::Binary;
    }
    ContentDiff::Lines(unified(
        path,
        before.is_some(),
        after.is_some(),
        before_content,
        after_content,
    ))
}

/// One changed run: the lines it replaces and the lines that replace them.
struct Hunk {
    before: Range<usize>,
    after: Range<usize>,
}

fn unified(path: &Path, has_before: bool, has_after: bool, before: &[u8], after: &[u8]) -> String {
    let before_lines: Vec<&[u8]> = byte_lines(before).collect();
    let after_lines: Vec<&[u8]> = byte_lines(after).collect();
    let input = InternedInput::new(before, after);
    let hunks: Vec<Hunk> = Diff::compute(Algorithm::Histogram, &input)
        .hunks()
        .map(|hunk| Hunk {
            before: widen(&hunk.before),
            after: widen(&hunk.after),
        })
        .collect();

    let mut out = String::new();
    if hunks.is_empty() {
        return out;
    }
    out.push_str(&format!(
        "--- {}\n+++ {}\n",
        side_name("a", path, has_before),
        side_name("b", path, has_after)
    ));
    for group in group_hunks(&hunks) {
        write_group(&mut out, group, &before_lines, &after_lines);
    }
    out
}

fn side_name(prefix: &str, path: &Path, present: bool) -> String {
    if present {
        format!("{prefix}/{path}")
    } else {
        "/dev/null".to_owned()
    }
}

fn widen(range: &Range<u32>) -> Range<usize> {
    range.start as usize..range.end as usize
}

/// Splits the hunks into runs whose context would touch or overlap, so each run is one `@@` hunk.
fn group_hunks(hunks: &[Hunk]) -> Vec<&[Hunk]> {
    let mut groups = Vec::new();
    let mut start = 0;
    for (index, pair) in hunks.windows(2).enumerate() {
        let [earlier, later] = pair else { continue };
        if later.before.start.saturating_sub(earlier.before.end) > 2 * CONTEXT {
            groups.push(hunks.get(start..=index).unwrap_or_default());
            start = index + 1;
        }
    }
    groups.push(hunks.get(start..).unwrap_or_default());
    groups
}

fn write_group(out: &mut String, group: &[Hunk], before: &[&[u8]], after: &[&[u8]]) {
    let (Some(first), Some(last)) = (group.first(), group.last()) else {
        return;
    };
    let lead = first.before.start.min(CONTEXT);
    let before_start = first.before.start - lead;
    let before_end = (last.before.end + CONTEXT).min(before.len());
    let after_start = first.after.start - lead;
    let after_end = last.after.end + (before_end - last.before.end);
    out.push_str(&format!(
        "@@ -{} +{} @@\n",
        range_header(before_start, before_end - before_start),
        range_header(after_start, after_end - after_start)
    ));
    let mut position = before_start;
    for hunk in group {
        push_lines(out, ' ', before, position..hunk.before.start);
        push_lines(out, '-', before, hunk.before.clone());
        push_lines(out, '+', after, hunk.after.clone());
        position = hunk.before.end;
    }
    push_lines(out, ' ', before, position..before_end);
}

/// The `start,length` of a hunk side, counting lines from one; an empty side names the line before.
fn range_header(start: usize, length: usize) -> String {
    match length {
        0 => format!("{start},0"),
        1 => format!("{}", start + 1),
        _ => format!("{},{length}", start + 1),
    }
}

fn push_lines(out: &mut String, marker: char, lines: &[&[u8]], range: Range<usize>) {
    for line in lines.iter().skip(range.start).take(range.len()) {
        out.push(marker);
        out.push_str(&String::from_utf8_lossy(line));
        if !line.ends_with(b"\n") {
            out.push_str("\n\\ No newline at end of file\n");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path() -> Path {
        "a.txt".parse().expect("a valid path")
    }

    fn lines(before: Option<&str>, after: Option<&str>) -> String {
        match diff(
            &path(),
            before.map(|text| (FileKind::Regular, text.as_bytes())),
            after.map(|text| (FileKind::Regular, text.as_bytes())),
        ) {
            ContentDiff::Lines(text) => text,
            other @ (ContentDiff::Binary | ContentDiff::KindOnly) => {
                panic!("expected a line diff, got {other:?}")
            }
        }
    }

    #[test]
    fn a_changed_line_shows_as_a_removal_and_an_addition_with_context() {
        let text = lines(Some("one\ntwo\nthree\n"), Some("one\n2\nthree\n"));
        assert_eq!(
            text,
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,3 +1,3 @@\n one\n-two\n+2\n three\n"
        );
    }

    #[test]
    fn an_added_file_diffs_against_dev_null() {
        let text = lines(None, Some("x\ny\n"));
        assert_eq!(
            text,
            "--- /dev/null\n+++ b/a.txt\n@@ -0,0 +1,2 @@\n+x\n+y\n"
        );
    }

    #[test]
    fn a_deleted_file_diffs_to_dev_null() {
        let text = lines(Some("x\n"), None);
        assert_eq!(text, "--- a/a.txt\n+++ /dev/null\n@@ -1 +0,0 @@\n-x\n");
    }

    #[test]
    fn a_missing_final_newline_is_marked() {
        let text = lines(Some("a\nb"), Some("a\nc"));
        assert_eq!(
            text,
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n a\n-b\n\\ No newline at end of file\n+c\n\\ No newline at end of file\n"
        );
    }

    #[test]
    fn distant_changes_make_separate_hunks() {
        let before: String = (1..=20).map(|n| format!("{n}\n")).collect();
        let after = before.replace("2\n", "two\n").replace("19\n", "nineteen\n");
        let text = lines(Some(&before), Some(&after));
        assert_eq!(text.matches("@@ -").count(), 2);
    }

    #[test]
    fn near_changes_share_one_hunk() {
        let before: String = (1..=8).map(|n| format!("{n}\n")).collect();
        let after = before.replace("2\n", "two\n").replace("6\n", "six\n");
        let text = lines(Some(&before), Some(&after));
        assert_eq!(text.matches("@@ -").count(), 1);
    }

    #[test]
    fn content_with_a_nul_byte_is_binary() {
        let result = diff(
            &path(),
            Some((FileKind::Regular, b"text\n")),
            Some((FileKind::Regular, b"bin\0ary")),
        );
        assert_eq!(result, ContentDiff::Binary);
    }

    #[test]
    fn content_that_is_not_utf8_is_binary() {
        let result = diff(
            &path(),
            Some((FileKind::Regular, b"text\n")),
            Some((FileKind::Regular, &[0xff, 0xfe])),
        );
        assert_eq!(result, ContentDiff::Binary);
    }

    #[test]
    fn the_same_content_in_another_kind_is_marked_kind_only() {
        let result = diff(
            &path(),
            Some((FileKind::Regular, b"same\n")),
            Some((FileKind::Executable, b"same\n")),
        );
        assert_eq!(result, ContentDiff::KindOnly);
    }
}
