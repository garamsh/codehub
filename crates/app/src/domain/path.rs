//! Repository paths: non-empty lists of components, ordered component by component.

use std::fmt;
use std::num::NonZeroUsize;
use std::str::FromStr;

/// Why bytes are not a component or components are not a path.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathError {
    /// A path has no components.
    Empty,
    /// A component is empty, holds `/` or NUL, or is `.` or `..`.
    InvalidComponent,
}

impl fmt::Display for PathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("a path has at least one component"),
            Self::InvalidComponent => f.write_str(
                "a component is non-empty, holds no '/' or NUL, and is neither '.' nor '..'",
            ),
        }
    }
}

impl std::error::Error for PathError {}

/// One name in a path. Components order by unsigned bytes, a proper prefix first.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Component(Vec<u8>);

impl Component {
    /// Parses `bytes` as a component.
    ///
    /// # Errors
    ///
    /// [`PathError::InvalidComponent`] where `bytes` is empty, holds `/` or NUL, or is `.` or `..`.
    pub fn new(bytes: Vec<u8>) -> Result<Self, PathError> {
        let valid = !bytes.is_empty()
            && !bytes.contains(&b'/')
            && !bytes.contains(&0)
            && bytes != b"."
            && bytes != b"..";
        if valid {
            Ok(Self(bytes))
        } else {
            Err(PathError::InvalidComponent)
        }
    }

    /// The component's bytes.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// A path from the repository root down. Paths order component by component, a proper prefix first.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Path(Vec<Component>);

impl Path {
    /// Builds a path from its components.
    ///
    /// # Errors
    ///
    /// [`PathError::Empty`] where `components` is empty.
    pub fn new(components: Vec<Component>) -> Result<Self, PathError> {
        if components.is_empty() {
            Err(PathError::Empty)
        } else {
            Ok(Self(components))
        }
    }

    /// The path's components, from the root down.
    #[must_use]
    pub fn components(&self) -> &[Component] {
        &self.0
    }

    /// The first `len` components of this path.
    pub(crate) fn truncated(&self, len: NonZeroUsize) -> Self {
        Self(self.0.iter().take(len.get()).cloned().collect())
    }
}

/// A directory of the repository, or the whole of it: the path a grant covers.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Prefix(Vec<Component>);

impl Prefix {
    /// The prefix covering the whole repository.
    #[must_use]
    pub const fn whole() -> Self {
        Self(Vec::new())
    }

    /// Whether `path` is this prefix or lies under it; components match whole, so `src` covers
    /// `src/a` and not `srcx/a`.
    #[must_use]
    pub fn covers(&self, path: &Path) -> bool {
        path.0.starts_with(&self.0)
    }
}

impl From<Path> for Prefix {
    fn from(path: Path) -> Self {
        Self(path.0)
    }
}

impl FromStr for Path {
    type Err = PathError;

    /// Parses `/`-separated components, such as `src/main.rs`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let components = s
            .split('/')
            .map(|part| Component::new(part.as_bytes().to_vec()))
            .collect::<Result<Vec<_>, _>>()?;
        Self::new(components)
    }
}

impl fmt::Display for Path {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (i, component) in self.0.iter().enumerate() {
            if i > 0 {
                f.write_str("/")?;
            }
            f.write_str(&String::from_utf8_lossy(component.as_bytes()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_slash_nul_dot_and_dot_dot_components() {
        assert!(Component::new(b"main.rs".to_vec()).is_ok());
        for bad in [&b""[..], b"a/b", b"a\0b", b".", b".."] {
            assert_eq!(
                Component::new(bad.to_vec()),
                Err(PathError::InvalidComponent)
            );
        }
    }

    #[test]
    fn rejects_a_path_with_no_components() {
        assert!(Path::new(vec![Component::new(b"a".to_vec()).unwrap()]).is_ok());
        assert_eq!(Path::new(Vec::new()), Err(PathError::Empty));
    }

    #[test]
    fn a_prefix_covers_whole_components_only() {
        let parse = |s: &str| s.parse::<Path>().unwrap();
        let src = Prefix::from(parse("src"));
        assert!(src.covers(&parse("src")));
        assert!(src.covers(&parse("src/a")));
        assert!(!src.covers(&parse("srcx/a")));
        assert!(!src.covers(&parse("docs/src")));
        assert!(Prefix::whole().covers(&parse("docs/b")));
    }

    #[test]
    fn orders_paths_component_by_component_with_a_prefix_first() {
        let parse = |s: &str| s.parse::<Path>().unwrap();
        // '/' (0x2F) sorts after '.' (0x2E) as bytes, but "a" is a prefix of "a.c".
        assert!(parse("a/b") < parse("a.c"));
        assert!(parse("a") < parse("a/b"));
        assert!(parse("ab") > parse("a/z"));
    }
}
