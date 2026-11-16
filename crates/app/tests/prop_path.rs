//! Paths, names and addresses: parsing and display round-trip, and a prefix covers whole
//! components only.

#[cfg(test)]
pub mod generators;

use app::domain::address::Address;
use app::domain::name::{CheckName, DraftName, Label, OrganisationName, RepositoryName};
use app::domain::path::{Component, Path, PathError, Prefix};
use proptest::prelude::*;

use generators::{address, narrow_path, wide_component};

/// A component of printable text, so that it survives being written and read back as text.
#[cfg(test)]
fn text_component() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9._ é-]{1,6}".prop_filter("not . or ..", |part| part != "." && part != "..")
}

/// A name the rules accept.
#[cfg(test)]
fn name_text() -> impl Strategy<Value = String> {
    "[a-z0-9][a-z0-9-]{0,62}"
}

/// Whether `text` is a name by the rules written out: 1 to 63 bytes of lowercase ASCII letters,
/// digits and hyphens, not starting with a hyphen.
#[cfg(test)]
fn is_name(text: &str) -> bool {
    (1..=63).contains(&text.len())
        && !text.starts_with('-')
        && text
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

proptest! {
    #[test]
    fn a_path_parses_from_what_it_displays(
        parts in prop::collection::vec(text_component(), 1..4),
    ) {
        let text = parts.join("/");
        let path: Path = text.parse().unwrap();
        prop_assert_eq!(path.to_string(), text);
        prop_assert_eq!(path.to_string().parse::<Path>(), Ok(path));
    }

    #[test]
    fn text_parses_to_a_path_exactly_where_every_component_is_valid(text in "[a-b./\0]{0,8}") {
        let valid = text
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != ".." && !part.contains('\0'));
        match text.parse::<Path>() {
            Ok(path) => {
                prop_assert!(valid);
                prop_assert_eq!(path.to_string(), text);
            }
            Err(error) => {
                prop_assert!(!valid);
                prop_assert_eq!(error, PathError::InvalidComponent);
            }
        }
    }

    #[test]
    fn a_prefix_covers_the_paths_that_start_with_its_components(
        path in narrow_path(),
        keep in 0_usize..4,
    ) {
        let keep = keep.min(path.components().len());
        let prefix = Prefix::new(path.components().iter().take(keep).cloned().collect());
        prop_assert!(prefix.covers(&path));
    }

    #[test]
    fn a_prefix_does_not_cover_a_path_one_of_whose_components_only_starts_alike(
        path in narrow_path(),
        at in 0_usize..3,
        suffix in wide_component(),
    ) {
        let at = at % path.components().len();
        let mut components: Vec<Component> = path.components().iter().take(at + 1).cloned().collect();
        let mut longer = components.pop().unwrap().as_bytes().to_vec();
        longer.extend_from_slice(suffix.as_bytes());
        let longer = Component::new(longer).unwrap();
        components.push(longer);
        let prefix = Prefix::new(components);
        prop_assert!(!prefix.covers(&path));
        // The control: the prefix with the component itself covers it.
        let own = Prefix::new(path.components().iter().take(at + 1).cloned().collect());
        prop_assert!(own.covers(&path));
    }

    #[test]
    fn a_prefix_does_not_cover_a_path_whose_component_it_only_starts(
        head in narrow_path(),
        component in wide_component(),
        suffix in wide_component(),
    ) {
        let mut longer = component.as_bytes().to_vec();
        longer.extend_from_slice(suffix.as_bytes());
        let mut path = head.components().to_vec();
        path.push(Component::new(longer).unwrap());
        let path = Path::new(path).unwrap();
        let mut prefix = head.components().to_vec();
        prefix.push(component);
        prop_assert!(!Prefix::new(prefix).covers(&path));
        prop_assert!(Prefix::new(head.components().to_vec()).covers(&path));
    }

    #[test]
    fn a_prefix_does_not_cover_a_path_shorter_than_itself(path in narrow_path(), extra in wide_component()) {
        let mut components = path.components().to_vec();
        components.push(extra);
        prop_assert!(!Prefix::new(components).covers(&path));
        prop_assert!(Prefix::whole().covers(&path));
    }

    #[test]
    fn paths_order_component_by_component_with_a_prefix_first(
        path in narrow_path(),
        other in narrow_path(),
    ) {
        prop_assert_eq!(path.cmp(&other), path.components().cmp(other.components()));
        let shorter = Path::new(path.components().iter().take(1).cloned().collect()).unwrap();
        if shorter != path {
            prop_assert!(shorter < path);
        }
    }

    #[test]
    fn a_name_parses_from_what_it_displays(text in name_text()) {
        prop_assert_eq!(text.parse::<RepositoryName>().unwrap().to_string(), text.clone());
        let organisation: OrganisationName = text.parse().unwrap();
        prop_assert_eq!(organisation.as_str(), text);
    }

    #[test]
    fn text_is_a_name_exactly_where_the_rules_accept_it(
        text in prop_oneof!["[a-z0-9-]{0,65}", "[a-zA-Z0-9_ é-]{0,8}"],
    ) {
        let expected = is_name(&text);
        prop_assert_eq!(text.parse::<OrganisationName>().is_ok(), expected);
        prop_assert_eq!(text.parse::<RepositoryName>().is_ok(), expected);
        prop_assert_eq!(text.parse::<DraftName>().is_ok(), expected);
        prop_assert_eq!(text.parse::<CheckName>().is_ok(), expected);
        prop_assert_eq!(text.parse::<Label>().is_ok(), expected);
    }

    #[test]
    fn an_address_parses_from_what_it_displays(address in address()) {
        let text = address.to_string();
        prop_assert_eq!(text.len(), "sha256:".len() + 64);
        prop_assert_eq!(text.parse::<Address>(), Ok(address));
    }

    #[test]
    fn text_that_parses_as_an_address_is_what_the_address_displays(
        text in prop_oneof!["sha256:[0-9a-fA-Fg]{62,66}", "(sha256:)?[0-9a-f]{0,8}"],
    ) {
        if let Ok(address) = text.parse::<Address>() {
            prop_assert_eq!(address.to_string(), text);
        }
    }
}
