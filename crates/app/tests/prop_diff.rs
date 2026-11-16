//! The line diff of a path between the base and a draft: what a reader applying it gets, and
//! when it says binary or kind-only.

use app::domain::diff::{ContentDiff, diff};
use app::domain::file::FileKind;
use app::domain::path::Path;
use proptest::collection::vec;
use proptest::prelude::*;

/// Text of up to eight newline-ended lines out of three.
fn text() -> impl Strategy<Value = String> {
    vec(prop::sample::select(vec!["a\n", "b\n", "c\n"]), 0..8).prop_map(|lines| lines.concat())
}

/// A before text, and an after text that is the same with a few lines changed, so that long
/// unchanged runs, which independent texts seldom have, come up.
fn edited() -> impl Strategy<Value = (String, String)> {
    (
        vec(prop::sample::select(vec!["a\n", "b\n", "c\n"]), 0..16),
        vec(
            (
                0_usize..16,
                0_usize..3,
                prop::sample::select(vec!["a\n", "b\n", "c\n"]),
            ),
            1..3,
        ),
    )
        .prop_map(|(before, edits)| {
            let mut after = before.clone();
            for (at, kind, line) in edits {
                let at = at.min(after.len());
                match (kind, at < after.len()) {
                    (0, _) => after.insert(at, line),
                    (1, true) => {
                        after.remove(at);
                    }
                    (_, true) => {
                        if let Some(slot) = after.get_mut(at) {
                            *slot = line;
                        }
                    }
                    (_, false) => {}
                }
            }
            (before.concat(), after.concat())
        })
}

#[cfg(test)]
fn path() -> Path {
    "dir/f.txt".parse().expect("a valid path")
}

fn side(text: Option<&String>) -> Option<(FileKind, &[u8])> {
    text.map(|text| (FileKind::Regular, text.as_bytes()))
}

/// The text a unified diff turns `before` into, or why it does not apply.
fn apply(before: &str, unified: &str) -> Result<String, String> {
    let old: Vec<&str> = before.split_inclusive('\n').collect();
    let mut body = unified.split_inclusive('\n').skip(2).peekable();
    let mut out = String::new();
    let mut at = 0;
    while let Some(header) = body.next() {
        let spec = header
            .strip_prefix("@@ -")
            .and_then(|rest| rest.split(' ').next())
            .ok_or_else(|| format!("not a hunk header: {header:?}"))?;
        let (start, count) = spec.split_once(',').unwrap_or((spec, "1"));
        let start: usize = start
            .parse()
            .map_err(|_| format!("bad start in {header:?}"))?;
        let count: usize = count
            .parse()
            .map_err(|_| format!("bad count in {header:?}"))?;
        // With the default context, a hunk that covers no before line is the diff of an empty
        // before text, and the printer writes its start as 1 (not 0, as GNU diff does).
        if count == 0 && !old.is_empty() {
            return Err(format!(
                "an empty hunk in a text that is not empty: {header:?}"
            ));
        }
        let first = start.saturating_sub(1);
        while at < first {
            out.push_str(old.get(at).ok_or("a hunk starts past the end")?);
            at += 1;
        }
        while let Some(line) = body.next_if(|line| !line.starts_with("@@")) {
            let (mark, rest) = line.split_at(1);
            match mark {
                " " | "-" => {
                    if old.get(at) != Some(&rest) {
                        return Err(format!("line {at} is not {rest:?} in {unified:?}"));
                    }
                    if mark == " " {
                        out.push_str(rest);
                    }
                    at += 1;
                }
                "+" => out.push_str(rest),
                _ => return Err(format!("not a diff line: {line:?}")),
            }
        }
    }
    while let Some(line) = old.get(at) {
        out.push_str(line);
        at += 1;
    }
    Ok(out)
}

proptest! {
    #[test]
    fn applying_the_diff_of_an_edited_text_gives_the_edited_text((before, after) in edited()) {
        prop_assume!(before != after);
        let ContentDiff::Lines(unified) = diff(&path(), side(Some(&before)), side(Some(&after))) else {
            return Err(TestCaseError::fail("a changed text is a line diff"));
        };
        prop_assert_eq!(apply(&before, &unified), Ok(after));
    }

    #[test]
    fn applying_the_diff_to_the_before_text_gives_the_after_text(
        before in prop::option::of(text()),
        after in prop::option::of(text()),
    ) {
        let ContentDiff::Lines(unified) = diff(&path(), side(before.as_ref()), side(after.as_ref())) else {
            // Both present and equal is the only text case that is not a line diff.
            prop_assert_eq!(&before, &after);
            return Ok(());
        };
        let head = format!(
            "--- {}\n+++ {}\n",
            if before.is_some() { "a/dir/f.txt" } else { "/dev/null" },
            if after.is_some() { "b/dir/f.txt" } else { "/dev/null" },
        );
        prop_assert!(unified.starts_with(&head));
        prop_assert_eq!(
            apply(before.as_deref().unwrap_or(""), &unified),
            Ok(after.unwrap_or_default())
        );
    }

    #[test]
    fn a_text_against_itself_differs_in_nothing_but_kind(text in text()) {
        let same = Some((FileKind::Regular, text.as_bytes()));
        prop_assert_eq!(diff(&path(), same, same), ContentDiff::KindOnly);
        let executable = Some((FileKind::Executable, text.as_bytes()));
        prop_assert_eq!(diff(&path(), same, executable), ContentDiff::KindOnly);
    }

    #[test]
    fn a_side_that_is_not_text_makes_the_diff_a_marker(text in text(), at in 0_usize..8) {
        let mut bytes = text.into_bytes();
        let at = at.min(bytes.len());
        bytes.insert(at, 0);
        let binary = Some((FileKind::Regular, bytes.as_slice()));
        let plain = Some((FileKind::Regular, &b"a\n"[..]));
        prop_assert_eq!(diff(&path(), binary, plain), ContentDiff::Binary);
        prop_assert_eq!(diff(&path(), plain, binary), ContentDiff::Binary);
    }
}
