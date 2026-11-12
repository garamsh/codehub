//! Reads a repository's check definitions from the TOML file `.codehub/checks.toml` (ADR 0035).
//!
//! The file lists each check as a `[[check]]` table:
//!
//! ```toml
//! [[check]]
//! name = "unit"            # required: 1 to 63 bytes of a-z, 0-9 and "-"
//! command = "cargo test"   # required: not blank
//! labels = ["linux"]       # optional: names a runner must offer
//! blocking = true          # optional: true unless it says otherwise
//! timeout = 600            # optional: seconds, 1 to 86400; 600 unless it says otherwise
//! ```

use std::collections::BTreeSet;
use std::ops::Range;

use app::domain::check::{CheckDefinition, Command, Timeout};
use app::domain::name::{CheckName, Label};
use app::domain::path::{Path, PathError};
use app::port::driven::check_definitions::{CheckDefinitions, CheckFileError};
use toml::Spanned;
use toml::de::{DeTable, DeValue};

/// Where a repository keeps its check definitions.
const FILE: &str = ".codehub/checks.toml";

/// The check definitions of a repository, read from the TOML at [`FILE`].
pub struct TomlCheckDefinitions {
    file: Path,
}

impl TomlCheckDefinitions {
    /// A reader of the file `.codehub/checks.toml`.
    ///
    /// # Errors
    ///
    /// [`PathError`] where the file's path is not a valid path, which it always is.
    pub fn new() -> Result<Self, PathError> {
        Ok(Self {
            file: FILE.parse()?,
        })
    }
}

/// The line of `text` that byte `offset` is on, counting from 1.
fn line_of(text: &str, offset: usize) -> usize {
    text.as_bytes().get(..offset).map_or(1, |before| {
        before.iter().filter(|byte| **byte == b'\n').count() + 1
    })
}

/// Reads one `[[check]]` table; `at` is where it starts.
struct Reader<'t> {
    text: &'t str,
}

impl Reader<'_> {
    fn invalid(&self, span: &Range<usize>, reason: impl Into<String>) -> CheckFileError {
        CheckFileError::Invalid {
            line: line_of(self.text, span.start),
            reason: reason.into(),
        }
    }

    fn string<'v>(
        &self,
        value: &'v Spanned<DeValue<'_>>,
        field: &str,
    ) -> Result<&'v str, CheckFileError> {
        value
            .get_ref()
            .as_str()
            .ok_or_else(|| self.invalid(&value.span(), format!("{field} is a string")))
    }

    fn check(&self, table: &Spanned<DeValue<'_>>) -> Result<CheckDefinition, CheckFileError> {
        let span = table.span();
        let fields = table
            .get_ref()
            .as_table()
            .ok_or_else(|| self.invalid(&span, "a check is a table"))?;
        for (key, _) in fields {
            if !matches!(
                key.get_ref().as_ref(),
                "name" | "command" | "labels" | "blocking" | "timeout"
            ) {
                return Err(self.invalid(&key.span(), format!("unknown field {}", key.get_ref())));
            }
        }
        let required = |field: &str| {
            fields
                .get(field)
                .ok_or_else(|| self.invalid(&span, format!("a check has a {field}")))
        };

        let name = self.string(required("name")?, "name")?;
        let command = self.string(required("command")?, "command")?;
        let name: CheckName = name
            .parse()
            .map_err(|error| self.invalid(&span, format!("name: {error}")))?;
        let command = Command::new(command)
            .map_err(|error| self.invalid(&span, format!("command: {error}")))?;

        let mut labels = BTreeSet::new();
        if let Some(value) = fields.get("labels") {
            let items = value
                .get_ref()
                .as_array()
                .ok_or_else(|| self.invalid(&value.span(), "labels is an array of strings"))?;
            for item in items {
                let label: Label = self
                    .string(item, "a label")?
                    .parse()
                    .map_err(|error| self.invalid(&item.span(), format!("label: {error}")))?;
                labels.insert(label);
            }
        }

        let blocking = match fields.get("blocking") {
            None => true,
            Some(value) => value
                .get_ref()
                .as_bool()
                .ok_or_else(|| self.invalid(&value.span(), "blocking is true or false"))?,
        };

        let timeout = match fields.get("timeout") {
            None => Timeout::default_timeout(),
            Some(value) => {
                let seconds = value
                    .get_ref()
                    .as_integer()
                    .and_then(|integer| integer.as_str().parse::<u64>().ok())
                    .ok_or_else(|| {
                        self.invalid(&value.span(), "timeout is a whole number of seconds")
                    })?;
                Timeout::new(seconds)
                    .map_err(|error| self.invalid(&value.span(), format!("timeout: {error}")))?
            }
        };

        Ok(CheckDefinition {
            name,
            command,
            labels,
            blocking,
            timeout,
        })
    }
}

impl CheckDefinitions for TomlCheckDefinitions {
    fn file(&self) -> &Path {
        &self.file
    }

    fn parse(&self, bytes: &[u8]) -> Result<Vec<CheckDefinition>, CheckFileError> {
        let text = std::str::from_utf8(bytes).map_err(|error| CheckFileError::Syntax {
            line: bytes.get(..error.valid_up_to()).map_or(1, |valid| {
                valid.iter().filter(|byte| **byte == b'\n').count() + 1
            }),
            description: "the file is not UTF-8".to_owned(),
        })?;
        let document = DeTable::parse(text).map_err(|error| CheckFileError::Syntax {
            line: error.span().map_or(1, |span| line_of(text, span.start)),
            description: error.message().to_owned(),
        })?;
        let reader = Reader { text };
        let root = document.get_ref();
        if let Some((key, _)) = root
            .iter()
            .find(|(key, _)| key.get_ref().as_ref() != "check")
        {
            return Err(reader.invalid(&key.span(), format!("unknown key {}", key.get_ref())));
        }
        let Some(checks) = root.get("check") else {
            return Ok(Vec::new());
        };
        let tables = checks
            .get_ref()
            .as_array()
            .ok_or_else(|| reader.invalid(&checks.span(), "check is an array of tables"))?;

        let mut seen = BTreeSet::new();
        let mut definitions = Vec::new();
        for table in tables {
            let definition = reader.check(table)?;
            if !seen.insert(definition.name.clone()) {
                return Err(CheckFileError::Duplicate {
                    line: line_of(text, table.span().start),
                    name: definition.name,
                });
            }
            definitions.push(definition);
        }
        Ok(definitions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Vec<CheckDefinition>, CheckFileError> {
        TomlCheckDefinitions::new().unwrap().parse(text.as_bytes())
    }

    #[test]
    fn reads_the_file_at_the_codehub_directory() {
        assert_eq!(
            TomlCheckDefinitions::new().unwrap().file().to_string(),
            ".codehub/checks.toml"
        );
    }

    #[test]
    fn reads_every_field_and_the_defaults() {
        let checks = parse(
            "[[check]]\nname = \"unit\"\ncommand = \"cargo test\"\nlabels = [\"linux\", \"gpu\"]\n\
             blocking = false\ntimeout = 30\n\n[[check]]\nname = \"lint\"\ncommand = \"make lint\"\n",
        )
        .unwrap();
        assert_eq!(checks.len(), 2);
        let unit = &checks[0];
        assert_eq!(unit.name.as_str(), "unit");
        assert_eq!(unit.command.as_str(), "cargo test");
        assert_eq!(unit.labels.len(), 2);
        assert!(!unit.blocking);
        assert_eq!(unit.timeout, Timeout::new(30).unwrap());
        let lint = &checks[1];
        assert!(lint.blocking);
        assert!(lint.labels.is_empty());
        assert_eq!(lint.timeout, Timeout::default_timeout());
    }

    #[test]
    fn a_file_with_no_check_defines_none() {
        assert_eq!(parse("").unwrap(), Vec::new());
    }

    #[test]
    fn rejects_a_duplicate_name_at_the_second_definition() {
        let error = parse(
            "[[check]]\nname = \"unit\"\ncommand = \"a\"\n\n[[check]]\nname = \"unit\"\ncommand = \"b\"\n",
        )
        .unwrap_err();
        assert_eq!(
            error,
            CheckFileError::Duplicate {
                line: 5,
                name: "unit".parse().unwrap()
            }
        );
    }

    #[test]
    fn rejects_an_empty_command() {
        let error = parse("[[check]]\nname = \"unit\"\ncommand = \"  \"\n").unwrap_err();
        assert!(
            matches!(error, CheckFileError::Invalid { line: 1, .. }),
            "{error:?}"
        );
        assert!(error.to_string().contains("command"));
    }

    #[test]
    fn rejects_a_timeout_out_of_range() {
        for seconds in ["0", "86401", "-5"] {
            let error = parse(&format!(
                "[[check]]\nname = \"unit\"\ncommand = \"a\"\ntimeout = {seconds}\n"
            ))
            .unwrap_err();
            assert!(
                matches!(error, CheckFileError::Invalid { line: 4, .. }),
                "{seconds}: {error:?}"
            );
            assert!(error.to_string().contains("timeout"), "{seconds}");
        }
    }

    #[test]
    fn rejects_a_bad_name_label_and_an_unknown_field() {
        for text in [
            "[[check]]\nname = \"Unit\"\ncommand = \"a\"\n",
            "[[check]]\nname = \"unit\"\ncommand = \"a\"\nlabels = [\"Linux\"]\n",
            "[[check]]\nname = \"unit\"\ncommand = \"a\"\nretries = 3\n",
            "[[check]]\nname = \"unit\"\n",
            "[[check]]\nname = \"unit\"\ncommand = \"a\"\nblocking = \"yes\"\n",
            "stray = 1\n",
        ] {
            let error = parse(text).unwrap_err();
            assert!(
                matches!(error, CheckFileError::Invalid { .. }),
                "{text}: {error:?}"
            );
        }
    }

    #[test]
    fn rejects_text_that_is_not_toml_naming_the_line() {
        let error = parse("[[check]]\nname = \n").unwrap_err();
        assert!(
            matches!(error, CheckFileError::Syntax { line: 2, .. }),
            "{error:?}"
        );
        let error = TomlCheckDefinitions::new()
            .unwrap()
            .parse(b"[[check]]\n\xff")
            .unwrap_err();
        assert!(
            matches!(error, CheckFileError::Syntax { line: 2, .. }),
            "{error:?}"
        );
    }
}
