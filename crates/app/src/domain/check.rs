//! Check definitions: the commands a repository asks to be run against each landed changeset
//! (ADR 0035), declared per folder with inputs, needs, outputs and caches (ADR 0049).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::str::FromStr;

use crate::domain::address::Address;
use crate::domain::name::{CheckName, Label};
use crate::domain::path::{Component, Path, Prefix};

/// The longest timeout a check may ask for, in seconds: one day.
const MAX_TIMEOUT: u64 = 86_400;

/// The timeout a check that names none has, in seconds.
const DEFAULT_TIMEOUT: u64 = 600;

/// Why text is not a check's command, timeout, input pattern or need.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CheckError {
    /// The command is empty or only whitespace.
    EmptyCommand,
    /// The timeout is not between 1 second and one day; carries what was asked.
    TimeoutRange(u64),
    /// The text is not an input pattern.
    BadPattern,
    /// The text is not a reference to a check.
    BadNeed,
}

impl fmt::Display for CheckError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyCommand => f.write_str("a command is not empty"),
            Self::TimeoutRange(seconds) => {
                write!(f, "a timeout is 1 to {MAX_TIMEOUT} seconds, not {seconds}")
            }
            Self::BadPattern => f.write_str(
                "a pattern is non-empty components joined by '/', each neither '.' nor '..', \
                 with '**' only as a whole component, after an optional leading '//'",
            ),
            Self::BadNeed => f.write_str(
                "a need is a check name, or '//<folder>:<name>' for a check of another folder, \
                 or '//:<name>' for a check at the root",
            ),
        }
    }
}

impl std::error::Error for CheckError {}

/// The command a check runs: the text a runner hands to its shell, never blank.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Command(String);

impl Command {
    /// The command `text`.
    ///
    /// # Errors
    ///
    /// [`CheckError::EmptyCommand`] where `text` holds only whitespace.
    pub fn new(text: &str) -> Result<Self, CheckError> {
        if text.trim().is_empty() {
            return Err(CheckError::EmptyCommand);
        }
        Ok(Self(text.to_owned()))
    }

    /// The command as text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// How long a check may run before it is timed out, in whole seconds from 1 to one day.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Timeout(u64);

impl Timeout {
    /// The timeout of `seconds`.
    ///
    /// # Errors
    ///
    /// [`CheckError::TimeoutRange`] where `seconds` is 0 or more than one day.
    pub const fn new(seconds: u64) -> Result<Self, CheckError> {
        if seconds == 0 || seconds > MAX_TIMEOUT {
            return Err(CheckError::TimeoutRange(seconds));
        }
        Ok(Self(seconds))
    }

    /// The timeout of a check that names none.
    #[must_use]
    pub const fn default_timeout() -> Self {
        Self(DEFAULT_TIMEOUT)
    }

    /// The timeout in seconds.
    #[must_use]
    pub const fn seconds(self) -> u64 {
        self.0
    }
}

/// A path pattern selecting the files a check reads: relative to the check's folder, or to the
/// repository root where it starts with `//`.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Pattern {
    from_root: bool,
    text: String,
}

impl Pattern {
    /// The pattern that selects every file under the folder, which is a check's input where it
    /// names none.
    #[must_use]
    pub fn whole_folder() -> Self {
        Self {
            from_root: false,
            text: "**".to_owned(),
        }
    }

    /// Whether the pattern is relative to the repository root and not to the check's folder.
    #[must_use]
    pub const fn from_root(&self) -> bool {
        self.from_root
    }

    /// The pattern without its leading `//`.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether the pattern selects `path`, for a check of `folder`. A component of the pattern
    /// matches a component of the path whole, `*` standing for any run of bytes within one
    /// component, and a component `**` standing for any number of components, none included.
    #[must_use]
    pub fn selects(&self, folder: &Prefix, path: &Path) -> bool {
        let base = if self.from_root {
            &[]
        } else {
            folder.components()
        };
        let Some(below) = path.components().strip_prefix(base) else {
            return false;
        };
        let names: Vec<&[u8]> = below.iter().map(Component::as_bytes).collect();
        let parts: Vec<&str> = self.text.split('/').collect();
        parts_match(&parts, &names)
    }
}

/// Whether the pattern components `parts` match the path components `names`.
fn parts_match(parts: &[&str], names: &[&[u8]]) -> bool {
    let Some((part, rest)) = parts.split_first() else {
        return names.is_empty();
    };
    if *part == "**" {
        return (0..=names.len()).any(|skip| {
            names
                .get(skip..)
                .is_some_and(|tail| parts_match(rest, tail))
        });
    }
    names
        .split_first()
        .is_some_and(|(name, tail)| wildcard(part.as_bytes(), name) && parts_match(rest, tail))
}

/// Whether `name` matches `pattern`, in which `*` stands for any run of bytes.
fn wildcard(pattern: &[u8], name: &[u8]) -> bool {
    match pattern.split_first() {
        None => name.is_empty(),
        Some((b'*', rest)) => {
            (0..=name.len()).any(|skip| name.get(skip..).is_some_and(|tail| wildcard(rest, tail)))
        }
        Some((byte, rest)) => name
            .split_first()
            .is_some_and(|(first, tail)| first == byte && wildcard(rest, tail)),
    }
}

impl FromStr for Pattern {
    type Err = CheckError;

    fn from_str(text: &str) -> Result<Self, CheckError> {
        let (from_root, rest) = text
            .strip_prefix("//")
            .map_or((false, text), |rest| (true, rest));
        let well_formed = rest.split('/').all(|part| {
            !part.is_empty()
                && part != "."
                && part != ".."
                && !part.contains('\0')
                && (part == "**" || !part.contains("**"))
        });
        if !well_formed {
            return Err(CheckError::BadPattern);
        }
        Ok(Self {
            from_root,
            text: rest.to_owned(),
        })
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.from_root {
            f.write_str("//")?;
        }
        f.write_str(&self.text)
    }
}

/// A check as a `needs` entry names it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum Need {
    /// A check of the same folder as the one that needs it.
    Sibling(CheckName),
    /// A check of the folder `folder`, which is the whole repository for `//:<name>`.
    Elsewhere {
        /// The folder of the check.
        folder: Prefix,
        /// The check's name in its folder.
        name: CheckName,
    },
}

impl FromStr for Need {
    type Err = CheckError;

    fn from_str(text: &str) -> Result<Self, CheckError> {
        let Some(reference) = text.strip_prefix("//") else {
            return text
                .parse()
                .map(Self::Sibling)
                .map_err(|_| CheckError::BadNeed);
        };
        let (folder, name) = reference.rsplit_once(':').ok_or(CheckError::BadNeed)?;
        let components = if folder.is_empty() {
            Vec::new()
        } else {
            folder
                .split('/')
                .map(|part| Component::new(part.as_bytes().to_vec()))
                .collect::<Result<_, _>>()
                .map_err(|_| CheckError::BadNeed)?
        };
        Ok(Self::Elsewhere {
            folder: Prefix::new(components),
            name: name.parse().map_err(|_| CheckError::BadNeed)?,
        })
    }
}

impl fmt::Display for Need {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sibling(name) => write!(f, "{name}"),
            Self::Elsewhere { folder, name } => write!(f, "//{}:{name}", folder_text(folder)),
        }
    }
}

/// The folder as `/`-joined text; empty for the root.
fn folder_text(folder: &Prefix) -> String {
    folder
        .components()
        .iter()
        .map(|component| String::from_utf8_lossy(component.as_bytes()))
        .collect::<Vec<_>>()
        .join("/")
}

/// A check's identity in its repository: its folder and its name there. It shows as the bare name
/// at the root and as `<folder>:<name>` elsewhere.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CheckId {
    /// The folder whose `.codehub/checks.toml` declares the check.
    pub folder: Prefix,
    /// The name the check has in its folder.
    pub name: CheckName,
}

impl fmt::Display for CheckId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.folder.components().is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{}:{}", folder_text(&self.folder), self.name)
        }
    }
}

/// What a check's result depends on: a digest of its definition, the files its inputs select and
/// the keys and output trees of the checks it needs (ADR 0049). A check whose key passed need not
/// run again.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct CheckKey(Address);

impl CheckKey {
    /// The key whose digest is `address`.
    #[must_use]
    pub const fn from_address(address: Address) -> Self {
        Self(address)
    }

    /// The key's digest.
    #[must_use]
    pub const fn address(&self) -> Address {
        self.0
    }

    /// The key of `definition` given `inputs`, the path and content address of each file its
    /// patterns select in the landed tree, and `needed`, the key and output tree of each check it
    /// needs, in the order of the definition's `needs`. Neither list is reordered, so `inputs` is
    /// given sorted by path. The definition's caches are not part of the key: a cache makes a run
    /// faster and must not change its result.
    #[must_use]
    pub fn new(
        definition: &CheckDefinition,
        inputs: &[(Path, Address)],
        needed: &[(CheckKey, Address)],
    ) -> Self {
        let mut bytes = KeyBytes(Vec::new());
        bytes.text(&definition.id().to_string());
        bytes.text(definition.command.as_str());
        bytes.count(definition.labels.len());
        for label in &definition.labels {
            bytes.text(&label.to_string());
        }
        bytes.text(if definition.blocking {
            "blocking"
        } else {
            "free"
        });
        bytes.text(&definition.timeout.seconds().to_string());
        bytes.count(definition.inputs.len());
        for pattern in &definition.inputs {
            bytes.text(&pattern.to_string());
        }
        bytes.count(definition.needs.len());
        for need in definition.needed() {
            bytes.text(&need.to_string());
        }
        bytes.count(definition.outputs.len());
        for output in &definition.outputs {
            bytes.text(&output.to_string());
        }
        bytes.count(inputs.len());
        for (path, content) in inputs {
            bytes.text(&path.to_string());
            bytes.0.extend_from_slice(content.digest());
        }
        bytes.count(needed.len());
        for (key, output) in needed {
            bytes.0.extend_from_slice(key.0.digest());
            bytes.0.extend_from_slice(output.digest());
        }
        Self(Address::of(&bytes.0))
    }
}

impl fmt::Display for CheckKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// The bytes a key is the digest of, each variable-length field preceded by its length.
struct KeyBytes(Vec<u8>);

impl KeyBytes {
    fn count(&mut self, count: usize) {
        self.0
            .extend_from_slice(&u64::try_from(count).unwrap_or(u64::MAX).to_be_bytes());
    }

    fn text(&mut self, text: &str) {
        self.count(text.len());
        self.0.extend_from_slice(text.as_bytes());
    }
}

/// One check a repository defines.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CheckDefinition {
    /// The folder whose `.codehub/checks.toml` declares the check; the whole repository for the
    /// file at the root.
    pub folder: Prefix,
    /// Names the check within its folder.
    pub name: CheckName,
    /// What a runner runs.
    pub command: Command,
    /// What a runner must offer to take the check.
    pub labels: BTreeSet<Label>,
    /// Whether `last-green` waits for the check to pass.
    pub blocking: bool,
    /// How long the check may run.
    pub timeout: Timeout,
    /// The files the check reads.
    pub inputs: Vec<Pattern>,
    /// The checks that must pass before this one.
    pub needs: Vec<Need>,
    /// Paths, relative to the folder, that the check produces.
    pub outputs: Vec<Path>,
    /// Paths, relative to the folder, that the check uses as a build cache.
    pub caches: Vec<Path>,
}

impl CheckDefinition {
    /// A check at the root that reads its whole folder and needs, outputs and caches nothing.
    #[must_use]
    pub fn new(
        name: CheckName,
        command: Command,
        labels: BTreeSet<Label>,
        blocking: bool,
        timeout: Timeout,
    ) -> Self {
        Self {
            folder: Prefix::whole(),
            name,
            command,
            labels,
            blocking,
            timeout,
            inputs: vec![Pattern::whole_folder()],
            needs: Vec::new(),
            outputs: Vec::new(),
            caches: Vec::new(),
        }
    }

    /// The check's identity in its repository.
    #[must_use]
    pub fn id(&self) -> CheckId {
        CheckId {
            folder: self.folder.clone(),
            name: self.name.clone(),
        }
    }

    /// Whether the check reads the file at `path`: one of its input patterns selects it.
    #[must_use]
    pub fn reads(&self, path: &Path) -> bool {
        self.inputs
            .iter()
            .any(|pattern| pattern.selects(&self.folder, path))
    }

    /// The checks this one needs, each named by folder and name.
    pub fn needed(&self) -> impl Iterator<Item = CheckId> + '_ {
        self.needs.iter().map(|need| match need {
            Need::Sibling(name) => CheckId {
                folder: self.folder.clone(),
                name: name.clone(),
            },
            Need::Elsewhere { folder, name } => CheckId {
                folder: folder.clone(),
                name: name.clone(),
            },
        })
    }

    /// The blocking check that stands for the check definitions at `file` in `folder` that could
    /// not be read. It is recorded as failed, so that `last-green` does not move past a changeset
    /// whose checks nobody could read.
    #[must_use]
    pub fn unreadable(folder: &Prefix, file: &Path) -> Self {
        let command = if folder.components().is_empty() {
            file.to_string()
        } else {
            format!("{}/{file}", folder_text(folder))
        };
        Self::new(
            CheckName::unreadable(),
            Command(command),
            BTreeSet::new(),
            true,
            Timeout::default_timeout(),
        )
    }
}

/// What is wrong with how a set of checks name each other.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum NeedsFault {
    /// `check` needs `need`, which no check is.
    Missing {
        /// The check that names it.
        check: CheckId,
        /// What it names.
        need: CheckId,
    },
    /// `check` is on a cycle of needs.
    Cycle {
        /// The lowest-named check on the cycle.
        check: CheckId,
    },
}

impl NeedsFault {
    /// The check whose declaration is at fault.
    #[must_use]
    pub const fn check(&self) -> &CheckId {
        match self {
            Self::Missing { check, .. } | Self::Cycle { check } => check,
        }
    }
}

impl fmt::Display for NeedsFault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing { check, need } => {
                write!(f, "check {check} needs {need}, which is not a check")
            }
            Self::Cycle { check } => write!(f, "check {check} is on a cycle of needs"),
        }
    }
}

/// The first fault in how `definitions` need each other: a need naming no check, in the order the
/// definitions list them, or else a cycle of needs.
#[must_use]
pub fn needs_fault(definitions: &[CheckDefinition]) -> Option<NeedsFault> {
    let mut needs: BTreeMap<CheckId, BTreeSet<CheckId>> = BTreeMap::new();
    for definition in definitions {
        needs.insert(definition.id(), definition.needed().collect());
    }
    for definition in definitions {
        if let Some(need) = definition.needed().find(|need| !needs.contains_key(need)) {
            return Some(NeedsFault::Missing {
                check: definition.id(),
                need,
            });
        }
    }

    // Removing a check once everything it needs is removed leaves the checks on a cycle and
    // those that need one.
    let mut waiting: BTreeMap<&CheckId, usize> = needs
        .iter()
        .map(|(check, needed)| (check, needed.len()))
        .collect();
    let mut needed_by: BTreeMap<&CheckId, Vec<&CheckId>> = BTreeMap::new();
    for (check, needed) in &needs {
        for need in needed {
            needed_by.entry(need).or_default().push(check);
        }
    }
    let mut ready: Vec<&CheckId> = waiting
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(check, _)| *check)
        .collect();
    while let Some(done) = ready.pop() {
        waiting.remove(done);
        for dependant in needed_by.get(done).into_iter().flatten() {
            if let Some(count) = waiting.get_mut(dependant) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    ready.push(dependant);
                }
            }
        }
    }

    // Each check left needs one that is left, so following the needs returns to a check.
    let mut walk: Vec<&CheckId> = vec![*waiting.keys().next()?];
    loop {
        let last = *walk.last()?;
        let next = needs
            .get(last)?
            .iter()
            .find(|need| waiting.contains_key(need))?;
        if let Some(start) = walk.iter().position(|seen| *seen == next) {
            let lowest = walk.get(start..)?.iter().min()?;
            return Some(NeedsFault::Cycle {
                check: (*lowest).clone(),
            });
        }
        walk.push(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(folder: &str, name: &str, needs: &[&str]) -> CheckDefinition {
        let mut check = CheckDefinition::new(
            name.parse().unwrap(),
            Command::new("run").unwrap(),
            BTreeSet::new(),
            true,
            Timeout::default_timeout(),
        );
        if !folder.is_empty() {
            check.folder = Prefix::from(folder.parse::<Path>().unwrap());
        }
        check.needs = needs.iter().map(|need| need.parse().unwrap()).collect();
        check
    }

    fn file(path: &str) -> (Path, Address) {
        (path.parse().unwrap(), Address::of(path.as_bytes()))
    }

    #[test]
    fn a_pattern_selects_the_files_it_names_below_the_folder_or_the_root() {
        let folder = Prefix::from("svc".parse::<Path>().unwrap());
        let path = |text: &str| text.parse::<Path>().unwrap();
        let all: Pattern = "**".parse().unwrap();
        assert!(all.selects(&folder, &path("svc/a/b")));
        assert!(!all.selects(&folder, &path("other/a")));
        let rust: Pattern = "src/**/*.rs".parse().unwrap();
        assert!(rust.selects(&folder, &path("svc/src/lib.rs")));
        assert!(rust.selects(&folder, &path("svc/src/a/b/lib.rs")));
        assert!(!rust.selects(&folder, &path("svc/src/lib.toml")));
        assert!(!rust.selects(&folder, &path("svc/tests/lib.rs")));
        let lock: Pattern = "//Cargo.lock".parse().unwrap();
        assert!(lock.selects(&folder, &path("Cargo.lock")));
        assert!(!lock.selects(&folder, &path("svc/Cargo.lock")));
    }

    #[test]
    fn a_key_is_stable_and_follows_what_the_result_depends_on() {
        let check = named("", "unit", &[]);
        let inputs = [file("a"), file("b")];
        let key = CheckKey::new(&check, &inputs, &[]);
        assert_eq!(key, CheckKey::new(&check, &inputs, &[]));

        let mut other = named("", "unit", &[]);
        other.blocking = false;
        assert_ne!(key, CheckKey::new(&other, &inputs, &[]));
        assert_ne!(key, CheckKey::new(&named("", "lint", &[]), &inputs, &[]));
        assert_ne!(key, CheckKey::new(&check, &[file("a")], &[]));
        let changed = [file("a"), (inputs[1].0.clone(), Address::of(b"new"))];
        assert_ne!(key, CheckKey::new(&check, &changed, &[]));

        let needs = named("", "ship", &["unit"]);
        let before = CheckKey::new(&needs, &[], &[(key, Address::of(b"one"))]);
        assert_ne!(
            before,
            CheckKey::new(&needs, &[], &[(key, Address::of(b"two"))])
        );
        assert_ne!(before, CheckKey::new(&needs, &[], &[]));
    }

    #[test]
    fn a_cache_does_not_change_the_key() {
        let plain = named("", "unit", &[]);
        let mut cached = named("", "unit", &[]);
        cached.caches = vec!["target".parse().unwrap()];
        assert_eq!(
            CheckKey::new(&plain, &[], &[]),
            CheckKey::new(&cached, &[], &[])
        );
    }

    #[test]
    fn a_command_is_not_blank() {
        assert_eq!(Command::new("  \n"), Err(CheckError::EmptyCommand));
        assert_eq!(Command::new("make ci").unwrap().as_str(), "make ci");
    }

    #[test]
    fn a_timeout_is_from_one_second_to_one_day() {
        assert_eq!(Timeout::new(0), Err(CheckError::TimeoutRange(0)));
        assert_eq!(Timeout::new(86_401), Err(CheckError::TimeoutRange(86_401)));
        assert_eq!(Timeout::new(1).unwrap().seconds(), 1);
        assert_eq!(Timeout::new(86_400).unwrap().seconds(), 86_400);
    }

    #[test]
    fn the_unreadable_check_blocks_and_names_the_file() {
        let file: Path = ".codehub/checks".parse().unwrap();
        let check = CheckDefinition::unreadable(&Prefix::whole(), &file);
        assert!(check.blocking);
        assert_eq!(check.command.as_str(), ".codehub/checks");
        let folder = Prefix::from("svc/api".parse::<Path>().unwrap());
        let check = CheckDefinition::unreadable(&folder, &file);
        assert_eq!(check.command.as_str(), "svc/api/.codehub/checks");
    }

    #[test]
    fn a_pattern_is_relative_to_the_folder_or_to_the_root() {
        let folder: Pattern = "src/**/*.rs".parse().unwrap();
        assert!(!folder.from_root());
        assert_eq!(folder.to_string(), "src/**/*.rs");
        let root: Pattern = "//Cargo.lock".parse().unwrap();
        assert!(root.from_root());
        assert_eq!(root.as_str(), "Cargo.lock");
        assert_eq!(root.to_string(), "//Cargo.lock");
    }

    #[test]
    fn a_malformed_pattern_is_refused() {
        for text in [
            "", "//", "a//b", "a/", "/a", "../a", "a/./b", "a**", "**b/c",
        ] {
            assert_eq!(
                text.parse::<Pattern>(),
                Err(CheckError::BadPattern),
                "{text}"
            );
        }
    }

    #[test]
    fn a_need_names_a_sibling_or_a_check_of_a_folder() {
        assert_eq!(
            "unit".parse::<Need>(),
            Ok(Need::Sibling("unit".parse().unwrap()))
        );
        let lib: Need = "//lib:build".parse().unwrap();
        assert_eq!(lib.to_string(), "//lib:build");
        let root: Need = "//:unit".parse().unwrap();
        assert_eq!(
            root,
            Need::Elsewhere {
                folder: Prefix::whole(),
                name: "unit".parse().unwrap()
            }
        );
        assert_eq!(root.to_string(), "//:unit");
        for text in [
            "",
            "lib:build",
            "//lib",
            "//lib/:c",
            "//lib:Bad",
            "//a//b:c",
        ] {
            assert_eq!(text.parse::<Need>(), Err(CheckError::BadNeed), "{text}");
        }
    }

    #[test]
    fn a_check_is_named_for_its_folder_unless_it_is_at_the_root() {
        assert_eq!(named("", "unit", &[]).id().to_string(), "unit");
        assert_eq!(
            named("services/api", "test", &[]).id().to_string(),
            "services/api:test"
        );
    }

    #[test]
    fn needs_resolve_against_the_folder_of_the_check() {
        let check = named("lib", "test", &["build", "//:setup", "//app:build"]);
        let ids: Vec<String> = check.needed().map(|id| id.to_string()).collect();
        assert_eq!(ids, ["lib:build", "setup", "app:build"]);
    }

    #[test]
    fn a_need_naming_no_check_is_a_fault_of_the_check_that_names_it() {
        let checks = [named("", "setup", &[]), named("lib", "test", &["build"])];
        let fault = needs_fault(&checks).unwrap();
        assert_eq!(fault.check().to_string(), "lib:test");
        assert!(matches!(fault, NeedsFault::Missing { .. }));
        let fine = [named("", "setup", &[]), named("lib", "test", &["//:setup"])];
        assert_eq!(needs_fault(&fine), None);
    }

    #[test]
    fn a_cycle_is_a_fault_of_its_lowest_named_check() {
        let checks = [
            named("", "z", &["a"]),
            named("", "a", &["b"]),
            named("", "b", &["a"]),
        ];
        let fault = needs_fault(&checks).unwrap();
        assert_eq!(
            fault,
            NeedsFault::Cycle {
                check: checks[1].id()
            }
        );
        let own = [named("", "a", &["a"])];
        assert!(matches!(needs_fault(&own), Some(NeedsFault::Cycle { .. })));
        let chain = [
            named("", "a", &["b"]),
            named("", "b", &["c"]),
            named("", "c", &[]),
        ];
        assert_eq!(needs_fault(&chain), None);
    }
}
