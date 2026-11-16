//! Image names and the scopes a registry client asks for them (ADR 0053): parsed once, trusted
//! after.

use std::str::FromStr;

use crate::domain::name::{OrganisationName, RepositoryName};

/// The longest image name, in bytes (the OCI distribution specification's bound).
const MAX_LEN: usize = 255;

/// What a token may allow on an image.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageAction {
    /// Pull the repository's images.
    Pull,
    /// Push the repository's images.
    Push,
}

impl ImageAction {
    /// The action as a registry client writes it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Pull => "pull",
            Self::Push => "push",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "pull" => Some(Self::Pull),
            "push" => Some(Self::Push),
            _ => None,
        }
    }
}

/// Why text is not an image name.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ImageNameError {
    /// The name is longer than 255 bytes, or is not three or more components of the OCI
    /// grammar separated by `/`.
    Malformed,
    /// The first two components are not an organisation and a repository of codehub.
    NotARepository,
}

/// The name of an image: `<organisation>/<repository>/<name>`, where the name is one or more
/// `/` separated components, each of the OCI distribution grammar. Its first two components name
/// the repository the image belongs to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ImageName {
    organisation: OrganisationName,
    repository: RepositoryName,
}

impl ImageName {
    /// The organisation that holds the image's repository.
    #[must_use]
    pub const fn organisation(&self) -> &OrganisationName {
        &self.organisation
    }

    /// The repository the image belongs to.
    #[must_use]
    pub const fn repository(&self) -> &RepositoryName {
        &self.repository
    }
}

impl FromStr for ImageName {
    type Err = ImageNameError;

    fn from_str(text: &str) -> Result<Self, ImageNameError> {
        if text.len() > MAX_LEN {
            return Err(ImageNameError::Malformed);
        }
        let mut components = text.split('/');
        let (Some(first), Some(second), Some(_)) =
            (components.next(), components.next(), components.next())
        else {
            return Err(ImageNameError::Malformed);
        };
        if !text.split('/').all(component) {
            return Err(ImageNameError::Malformed);
        }
        match (first.parse(), second.parse()) {
            (Ok(organisation), Ok(repository)) => Ok(Self {
                organisation,
                repository,
            }),
            _ => Err(ImageNameError::NotARepository),
        }
    }
}

/// Whether `text` is `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*`.
fn component(text: &str) -> bool {
    let bytes = text.as_bytes();
    let alnum =
        |byte: Option<&u8>| byte.is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    let mut at = 0;
    loop {
        let start = at;
        while alnum(bytes.get(at)) {
            at += 1;
        }
        if at == start {
            return false;
        }
        match bytes.get(at) {
            None => return true,
            Some(b'.') => at += 1,
            Some(b'_') => {
                at += 1;
                if bytes.get(at) == Some(&b'_') {
                    at += 1;
                }
            }
            Some(b'-') => {
                while bytes.get(at) == Some(&b'-') {
                    at += 1;
                }
            }
            Some(_) => return false,
        }
    }
}

/// One scope of a token request, `<type>:<name>:<actions>`, as the client wrote it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Scope {
    /// What the name names: `repository` for an image's repository.
    pub kind: String,
    /// The name, as written.
    pub name: String,
    /// The actions asked for, as written, in order and without repeats or empty ones.
    pub actions: Vec<String>,
}

impl Scope {
    /// The scope `text` writes; `None` where it has no `<type>:<name>:<actions>` shape.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let (kind, rest) = text.split_once(':')?;
        let (name, actions) = rest.rsplit_once(':')?;
        let mut kept: Vec<String> = Vec::new();
        for action in actions.split(',').filter(|action| !action.is_empty()) {
            if !kept.iter().any(|held| held == action) {
                kept.push(action.to_owned());
            }
        }
        Some(Self {
            kind: kind.to_owned(),
            name: name.to_owned(),
            actions: kept,
        })
    }

    /// The image this scope names, where it is a `repository` scope of a well-formed image name.
    #[must_use]
    pub fn image(&self) -> Option<ImageName> {
        if self.kind == "repository" {
            self.name.parse().ok()
        } else {
            None
        }
    }

    /// Whether the scope asks for `action`.
    #[must_use]
    pub fn asks(&self, action: ImageAction) -> bool {
        self.actions
            .iter()
            .any(|asked| ImageAction::parse(asked) == Some(action))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_organisation_and_the_repository_of_an_image_name() {
        let name: ImageName = "acme/app/web".parse().unwrap();
        assert_eq!(name.organisation().as_str(), "acme");
        assert_eq!(name.repository().as_str(), "app");
        assert!("acme/app/web/sub/part".parse::<ImageName>().is_ok());
        assert!("acme/app/a.b_c__d-e--f".parse::<ImageName>().is_ok());
    }

    #[test]
    fn refuses_a_name_with_fewer_than_three_components() {
        for text in [
            "",
            "acme",
            "acme/app",
            "acme/app/",
            "/acme/app/web",
            "acme//web",
        ] {
            assert_eq!(
                text.parse::<ImageName>(),
                Err(ImageNameError::Malformed),
                "{text}"
            );
        }
    }

    #[test]
    fn refuses_a_component_outside_the_oci_grammar() {
        for text in [
            "acme/app/Web",
            "acme/app/-web",
            "acme/app/web-",
            "acme/app/we..b",
            "acme/app/we___b",
            "acme/app/.web",
            "acme/app/web.",
            "acme/app/web_",
            "acme/app/we b",
            "acme/app/web:1",
            "acme/app/web@sha256",
        ] {
            assert_eq!(
                text.parse::<ImageName>(),
                Err(ImageNameError::Malformed),
                "{text}"
            );
        }
        // Control: the separators the grammar allows between two runs of alphanumerics.
        for text in [
            "acme/app/a.b",
            "acme/app/a_b",
            "acme/app/a__b",
            "acme/app/a-b",
            "acme/app/a---b",
        ] {
            assert!(text.parse::<ImageName>().is_ok(), "{text}");
        }
    }

    #[test]
    fn refuses_a_name_whose_first_components_are_not_a_codehub_repository() {
        // A codehub name may end in `-`, which the OCI grammar forbids: such a repository has no
        // image names, so the grammar refuses it first.
        assert_eq!(
            "acme/app-/web".parse::<ImageName>(),
            Err(ImageNameError::Malformed)
        );
        // Components the grammar allows and a codehub name does not: a dot, an underscore.
        assert_eq!(
            "acme/a.b/web".parse::<ImageName>(),
            Err(ImageNameError::NotARepository)
        );
        assert_eq!(
            "a_b/app/web".parse::<ImageName>(),
            Err(ImageNameError::NotARepository)
        );
    }

    #[test]
    fn bounds_the_name_at_255_bytes() {
        let name = |length: usize| format!("acme/app/{}", "a".repeat(length - "acme/app/".len()));
        assert!(name(255).parse::<ImageName>().is_ok());
        assert_eq!(
            name(256).parse::<ImageName>(),
            Err(ImageNameError::Malformed)
        );
    }

    #[test]
    fn parses_a_scope_into_its_type_name_and_distinct_actions() {
        let scope = Scope::parse("repository:acme/app/web:pull,push,pull").unwrap();
        assert_eq!(scope.kind, "repository");
        assert_eq!(scope.name, "acme/app/web");
        assert_eq!(scope.actions, ["pull", "push"]);
        assert!(scope.asks(ImageAction::Pull) && scope.asks(ImageAction::Push));
        let scope = Scope::parse("repository:acme/app/web:*").unwrap();
        assert_eq!(scope.actions, ["*"]);
        assert!(!scope.asks(ImageAction::Pull) && !scope.asks(ImageAction::Push));
        assert!(
            Scope::parse("repository:acme/app/web:")
                .unwrap()
                .actions
                .is_empty()
        );
    }

    #[test]
    fn has_no_scope_in_text_without_the_three_parts() {
        assert_eq!(Scope::parse("repository"), None);
        assert_eq!(Scope::parse("repository:acme/app/web"), None);
        assert_eq!(Scope::parse(""), None);
    }

    #[test]
    fn names_an_image_only_for_a_repository_scope_of_a_well_formed_name() {
        let image = |text: &str| Scope::parse(text).unwrap().image();
        assert!(image("repository:acme/app/web:pull").is_some());
        assert!(image("registry:acme/app/web:pull").is_none());
        assert!(image("repository:acme/app:pull").is_none());
    }
}
