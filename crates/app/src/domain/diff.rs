//! The difference between a path's content in the base and in a draft (ADR 0048): a unified line
//! diff where both sides are text (ADR 0017), a marker otherwise, and the change of file kind
//! beside either.

use imara_diff::{Algorithm, BasicLineDiffPrinter, Diff, InternedInput, UnifiedDiffConfig};

use super::file::FileKind;
use super::merge::is_text;
use super::path::Path;

/// How a path's content differs between two sides.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ContentDiff {
    /// Both sides are text: the unified diff, headed by the two sides' names.
    Lines(String),
    /// A side is not text, and the content differs.
    Binary,
    /// Both sides hold the same content.
    KindOnly,
}

/// A side of a diff: the file's kind and its content.
pub type Side<'a> = Option<(FileKind, &'a [u8])>;

/// A path present on both sides whose file kind differs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct KindChange {
    /// The kind in the base.
    pub before: FileKind,
    /// The kind in the draft.
    pub after: FileKind,
}

/// The kind change from `before` to `after`, if both sides exist and their kinds differ.
#[must_use]
pub fn kind_change(before: Side<'_>, after: Side<'_>) -> Option<KindChange> {
    match (before, after) {
        (Some((before, _)), Some((after, _))) if before != after => {
            Some(KindChange { before, after })
        }
        _ => None,
    }
}

/// The difference of `path` from `before` to `after`; an absent side counts as empty text.
#[must_use]
pub fn diff(path: &Path, before: Side<'_>, after: Side<'_>) -> ContentDiff {
    let before_content = before.map_or(&[][..], |(_, content)| content);
    let after_content = after.map_or(&[][..], |(_, content)| content);
    if before.is_some() && after.is_some() && before_content == after_content {
        return ContentDiff::KindOnly;
    }
    let (Ok(before_text), Ok(after_text)) = (
        std::str::from_utf8(before_content),
        std::str::from_utf8(after_content),
    ) else {
        return ContentDiff::Binary;
    };
    if !is_text(before_content) || !is_text(after_content) {
        return ContentDiff::Binary;
    }
    let input = InternedInput::new(before_text, after_text);
    let mut diff = Diff::compute(Algorithm::Histogram, &input);
    diff.postprocess_lines(&input);
    let hunks = diff
        .unified_diff(
            &BasicLineDiffPrinter(&input.interner),
            UnifiedDiffConfig::default(),
            &input,
        )
        .to_string();
    ContentDiff::Lines(format!(
        "--- {}\n+++ {}\n{hunks}",
        side_name("a", path, before.is_some()),
        side_name("b", path, after.is_some())
    ))
}

fn side_name(prefix: &str, path: &Path, present: bool) -> String {
    if present {
        format!("{prefix}/{path}")
    } else {
        "/dev/null".to_owned()
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
            "--- /dev/null\n+++ b/a.txt\n@@ -1,0 +1,2 @@\n+x\n+y\n"
        );
    }

    #[test]
    fn a_deleted_file_diffs_to_dev_null() {
        let text = lines(Some("x\n"), None);
        assert_eq!(text, "--- a/a.txt\n+++ /dev/null\n@@ -1,1 +1,0 @@\n-x\n");
    }

    #[test]
    fn a_line_without_a_final_newline_is_diffed_like_any_other() {
        let text = lines(Some("a\nb"), Some("a\nc"));
        assert_eq!(
            text,
            "--- a/a.txt\n+++ b/a.txt\n@@ -1,2 +1,2 @@\n a\n-b\n+c\n"
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

    #[test]
    fn an_added_or_deleted_empty_file_still_has_its_header() {
        assert_eq!(lines(None, Some("")), "--- /dev/null\n+++ b/a.txt\n");
        assert_eq!(lines(Some(""), None), "--- a/a.txt\n+++ /dev/null\n");
    }

    #[test]
    fn a_kind_change_is_reported_whatever_the_content_does() {
        let regular = Some((FileKind::Regular, &b"one\n"[..]));
        let executable = Some((FileKind::Executable, &b"one\n"[..]));
        let edited = Some((FileKind::Executable, &b"two\n"[..]));
        let change = Some(KindChange {
            before: FileKind::Regular,
            after: FileKind::Executable,
        });
        assert_eq!(kind_change(regular, executable), change);
        assert_eq!(kind_change(regular, edited), change);
        assert_eq!(kind_change(regular, regular), None);
        assert_eq!(kind_change(None, executable), None);
        assert_eq!(kind_change(regular, None), None);
    }
}
