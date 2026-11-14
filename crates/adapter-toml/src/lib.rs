//! Reads a folder's check definitions from the TOML file `.codehub/checks.toml` (ADR 0035,
//! ADR 0049).
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
//! inputs = ["src/**"]      # optional: patterns, "//" for the root; the whole folder by default
//! needs = ["lint", "//lib:build"]  # optional: checks that run first
//! outputs = ["target/bin"] # optional: paths, relative to the folder, the check produces
//! caches = ["target"]      # optional: paths, relative to the folder, used as a build cache
//! ```

use std::collections::BTreeSet;
use std::ops::Range;

use app::domain::check::{CheckDefinition, Command, Need, Pattern, Timeout};
use app::domain::name::{CheckName, Label};
use app::domain::path::{Path, PathError, Prefix};
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

/// Reads the `[[check]]` tables of the file of `folder`.
struct Reader<'t> {
    text: &'t str,
    folder: &'t Prefix,
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

    /// The strings of the array `field`, each read by `parse`; `None` where `field` is absent.
    fn list<T>(
        &self,
        fields: &DeTable<'_>,
        field: &str,
        parse: impl Fn(&str) -> Result<T, String>,
    ) -> Result<Option<Vec<T>>, CheckFileError> {
        let Some(value) = fields.get(field) else {
            return Ok(None);
        };
        let items = value.get_ref().as_array().ok_or_else(|| {
            self.invalid(&value.span(), format!("{field} is an array of strings"))
        })?;
        items
            .iter()
            .map(|item| {
                parse(self.string(item, field)?)
                    .map_err(|error| self.invalid(&item.span(), format!("{field}: {error}")))
            })
            .collect::<Result<_, _>>()
            .map(Some)
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
                "name"
                    | "command"
                    | "labels"
                    | "blocking"
                    | "timeout"
                    | "inputs"
                    | "needs"
                    | "outputs"
                    | "caches"
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

        let inputs = self
            .list(fields, "inputs", |text| {
                text.parse::<Pattern>().map_err(|error| error.to_string())
            })?
            .unwrap_or_else(|| vec![Pattern::whole_folder()]);
        let needs = self
            .list(fields, "needs", |text| {
                text.parse::<Need>().map_err(|error| error.to_string())
            })?
            .unwrap_or_default();
        let path = |text: &str| text.parse::<Path>().map_err(|error| error.to_string());
        let outputs = self.list(fields, "outputs", path)?.unwrap_or_default();
        let caches = self.list(fields, "caches", path)?.unwrap_or_default();

        Ok(CheckDefinition {
            folder: self.folder.clone(),
            name,
            command,
            labels,
            blocking,
            timeout,
            inputs,
            needs,
            outputs,
            caches,
        })
    }
}

impl CheckDefinitions for TomlCheckDefinitions {
    fn file(&self) -> &Path {
        &self.file
    }

    fn parse(&self, folder: &Prefix, bytes: &[u8]) -> Result<Vec<CheckDefinition>, CheckFileError> {
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
        let reader = Reader { text, folder };
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
        parse_in(&Prefix::whole(), text)
    }

    fn parse_in(folder: &Prefix, text: &str) -> Result<Vec<CheckDefinition>, CheckFileError> {
        TomlCheckDefinitions::new()
            .unwrap()
            .parse(folder, text.as_bytes())
    }

    fn check_with(field: &str) -> String {
        format!("[[check]]\nname = \"unit\"\ncommand = \"a\"\n{field}\n")
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
            .parse(&Prefix::whole(), b"[[check]]\n\xff")
            .unwrap_err();
        assert!(
            matches!(error, CheckFileError::Syntax { line: 2, .. }),
            "{error:?}"
        );
    }

    #[test]
    fn reads_inputs_needs_outputs_and_caches() {
        let checks = parse(&check_with(
            "inputs = [\"src/**\", \"//Cargo.lock\"]\nneeds = [\"lint\", \"//lib:build\", \"//:unit\"]\n\
             outputs = [\"target/bin\"]\ncaches = [\"target\", \".cache/deps\"]",
        ))
        .unwrap();
        let unit = &checks[0];
        let inputs: Vec<String> = unit.inputs.iter().map(ToString::to_string).collect();
        assert_eq!(inputs, ["src/**", "//Cargo.lock"]);
        let needs: Vec<String> = unit.needs.iter().map(ToString::to_string).collect();
        assert_eq!(needs, ["lint", "//lib:build", "//:unit"]);
        let outputs: Vec<String> = unit.outputs.iter().map(ToString::to_string).collect();
        assert_eq!(outputs, ["target/bin"]);
        let caches: Vec<String> = unit.caches.iter().map(ToString::to_string).collect();
        assert_eq!(caches, ["target", ".cache/deps"]);
    }

    #[test]
    fn the_inputs_default_to_the_folder_and_the_rest_to_nothing() {
        let unit = parse(&check_with("")).unwrap().remove(0);
        assert_eq!(unit.inputs, [Pattern::whole_folder()]);
        assert!(unit.needs.is_empty());
        assert!(unit.outputs.is_empty());
        assert!(unit.caches.is_empty());
    }

    #[test]
    fn a_need_without_slashes_names_a_check_of_the_same_folder() {
        let unit = parse(&check_with("needs = [\"lint\"]")).unwrap().remove(0);
        assert_eq!(unit.needs, [Need::Sibling("lint".parse().unwrap())]);
    }

    #[test]
    fn the_folder_of_a_check_is_the_one_the_file_is_read_for() {
        let text = check_with("");
        let root = parse_in(&Prefix::whole(), &text).unwrap().remove(0);
        assert_eq!(root.folder, Prefix::whole());
        assert_eq!(root.id().to_string(), "unit");
        let folder = Prefix::from("svc/api".parse::<Path>().unwrap());
        let nested = parse_in(&folder, &text).unwrap().remove(0);
        assert_eq!(nested.folder, folder);
        assert_eq!(nested.id().to_string(), "svc/api:unit");
        assert_eq!(nested.name.as_str(), "unit");
    }

    #[test]
    fn rejects_a_malformed_pattern_need_or_path_naming_the_field() {
        for (field, text) in [
            ("inputs", "inputs = [\"\"]"),
            ("inputs", "inputs = [\"a//b\"]"),
            ("inputs", "inputs = [\"../x\"]"),
            ("inputs", "inputs = [\"a**\"]"),
            ("inputs", "inputs = [1]"),
            ("inputs", "inputs = \"src\""),
            ("needs", "needs = [\"//lib\"]"),
            ("needs", "needs = [\"Lint\"]"),
            ("needs", "needs = [\"//lib:\"]"),
            ("outputs", "outputs = [\"a/../b\"]"),
            ("outputs", "outputs = [\"\"]"),
            ("caches", "caches = [\"/abs\"]"),
        ] {
            let error = parse(&check_with(text)).unwrap_err();
            assert!(
                matches!(error, CheckFileError::Invalid { line: 4, .. }),
                "{text}: {error:?}"
            );
            assert!(error.to_string().contains(field), "{text}: {error}");
        }
    }

    #[test]
    fn a_pattern_need_and_path_that_are_well_formed_are_not_refused() {
        for text in [
            "inputs = [\"**\", \"a/**/b.rs\", \"//x/*\"]",
            "needs = [\"//:unit\", \"//a/b:c\"]",
            "outputs = [\"a/b\"]",
            "caches = [\"a\"]",
        ] {
            assert!(parse(&check_with(text)).is_ok(), "{text}");
        }
    }
}
