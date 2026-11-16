//! The text of each error and identifier a caller is shown, and the values its accessors hand
//! back: what an operator reads in a refusal and what a store is given.

use std::collections::BTreeMap;
use std::error::Error;

use app::domain::address::{Address, AddressError};
use app::domain::check::{CheckError, CheckKey, Need, Pattern, Timeout};
use app::domain::credential::{Credential, EmptyCredential};
use app::domain::derivation::DerivationError;
use app::domain::draft::{DraftId, Instance};
use app::domain::encoding::{DecodeError, EncodeError};
use app::domain::file::FileKind;
use app::domain::file_version::{FileVersionError, HistoryError};
use app::domain::job::JobId;
use app::domain::merge::{Change, FileConflict, FileVersion, rebase};
use app::domain::name::{DraftName, NameError};
use app::domain::path::{Path, PathError};
use app::domain::principal::{Principal, PrincipalError};
use app::domain::repository::RepositoryId;
use app::domain::run::RunId;
use app::domain::runner::RegistrationId;
use app::domain::tree::DeriveError;
use app::port::driving::revert::{QueueChecksError, RevertError};
use app::port::driving::source_control::LandError;
use app::use_case::source_control::AfterLandingError;

#[cfg(test)]
fn path(text: &str) -> Path {
    text.parse().unwrap()
}

#[cfg(test)]
fn version(text: &str) -> FileVersion {
    FileVersion {
        kind: FileKind::Regular,
        content: text.as_bytes().to_vec(),
    }
}

#[test]
fn an_address_error_says_what_is_wrong_with_the_text() {
    let digits = "g".repeat(64);
    assert_eq!(
        "abc".parse::<Address>().unwrap_err().to_string(),
        "an address starts with \"sha256:\""
    );
    assert_eq!(
        "sha256:ab".parse::<Address>().unwrap_err().to_string(),
        "an address has 64 hex digits after its prefix, not 2"
    );
    assert_eq!(
        format!("sha256:{digits}")
            .parse::<Address>()
            .unwrap_err()
            .to_string(),
        "an address holds lowercase hex digits, not byte 0x67"
    );
    assert_eq!(
        format!("sha256:{digits}").parse::<Address>(),
        Err(AddressError::InvalidDigit(b'g'))
    );
}

#[test]
fn an_address_is_written_the_same_for_a_person_and_for_a_log() {
    let address = Address::of(b"x");
    assert_eq!(format!("{address:?}"), address.to_string());
    assert!(address.to_string().starts_with("sha256:2d711642"));
}

#[test]
fn a_parsed_address_has_each_of_its_digits() {
    // The high and low digit of each byte are told apart: 0x1f is not 0xf1.
    let text = format!("sha256:1f{}", "00".repeat(31));
    let address: Address = text.parse().unwrap();
    assert_eq!(address.digest()[0], 0x1f);
    assert_eq!(address.to_string(), text);
}

#[test]
fn a_check_error_says_what_is_wrong_with_the_text() {
    assert_eq!(
        CheckError::EmptyCommand.to_string(),
        "a command is not empty"
    );
    assert_eq!(
        Timeout::new(0).unwrap_err().to_string(),
        "a timeout is 1 to 86400 seconds, not 0"
    );
    assert!(
        "".parse::<Pattern>()
            .unwrap_err()
            .to_string()
            .starts_with("a pattern is non-empty components")
    );
    assert!(
        "a:b:c"
            .parse::<Need>()
            .unwrap_err()
            .to_string()
            .starts_with("a need is a check name")
    );
}

#[test]
fn a_check_key_is_written_as_its_address() {
    let address = Address::of(b"key");
    assert_eq!(
        CheckKey::from_address(address).to_string(),
        address.to_string()
    );
}

#[test]
fn a_credential_is_shown_to_the_caller_and_hidden_from_a_log() {
    assert_eq!(
        "".parse::<Credential>().unwrap_err().to_string(),
        "a credential is not empty"
    );
    assert_eq!("".parse::<Credential>(), Err(EmptyCredential));
    let credential: Credential = "s3cret".parse().unwrap();
    assert_eq!(credential.as_str(), "s3cret");
    assert_eq!(format!("{credential:?}"), "Credential(..)");
}

#[test]
fn a_derivation_error_says_which_parents_are_missing_or_what_the_step_said() {
    let count: DerivationError<NameError> = DerivationError::ParentCount {
        expected: 2,
        given: 1,
    };
    assert_eq!(count.to_string(), "1 parent values for 2 parents");
    let step = DerivationError::Step(NameError::LeadingHyphen);
    assert_eq!(step.to_string(), NameError::LeadingHyphen.to_string());
}

#[test]
fn a_draft_and_its_instance_are_written_as_their_names_and_numbers() {
    let id = DraftId {
        principal: Principal::new("https://id", "ada").unwrap(),
        name: "work".parse::<DraftName>().unwrap(),
    };
    assert_eq!(id.to_string(), "https://id ada/work");
    assert_eq!(Instance::new(5).to_string(), "5");
    assert_eq!(Instance::new(5).get(), 5);
}

#[test]
fn an_encoding_error_says_what_is_wrong_with_the_bytes() {
    assert_eq!(
        EncodeError::TooLong(5).to_string(),
        "5 exceeds the u32 length prefix"
    );
    assert_eq!(
        DecodeError::UnexpectedEnd.to_string(),
        "input ends inside a field"
    );
    assert_eq!(
        DecodeError::NotSorted.to_string(),
        "entries are out of order or repeated"
    );
}

#[test]
fn a_file_version_error_names_the_path_or_the_node() {
    assert_eq!(
        FileVersionError::NotFound(path("a/b")).to_string(),
        "a/b has no file version to delete"
    );
    let address = Address::of(b"node");
    assert_eq!(
        HistoryError::MissingNode(address).to_string(),
        format!("file-version node {address} was not supplied")
    );
}

#[test]
fn identifiers_are_written_as_their_numbers_and_give_them_back() {
    assert_eq!(JobId::new(7).to_string(), "7");
    assert_eq!(JobId::new(7).value(), 7);
    assert_eq!(RunId::new(8).to_string(), "8");
    assert_eq!(RunId::new(8).value(), 8);
    assert_eq!(RegistrationId::new(9).to_string(), "9");
    assert_eq!(RegistrationId::new(9).value(), 9);
    assert_eq!(RepositoryId::new(10).to_string(), "10");
    assert_eq!(RepositoryId::new(10).value(), 10);
}

#[test]
fn a_refused_merge_counts_its_paths_and_says_why_each_conflicts() {
    let base = BTreeMap::from([("f", version("1\n2\n3\n"))]);
    let main = BTreeMap::from([("f", Change::Write(version("1\nA\n3\n")))]);
    let landing = BTreeMap::from([("f", Change::Write(version("1\nB\n3\n")))]);
    let conflicts = rebase(&base, &main, &landing).unwrap_err();
    assert_eq!(conflicts.to_string(), "1 conflicting paths");
    assert_eq!(conflicts.paths().len(), 1);

    assert_eq!(
        FileConflict::DeleteModify.to_string(),
        "deleted on one side and modified on the other"
    );
    assert_eq!(
        FileConflict::AddAdd.to_string(),
        "added on both sides with different content"
    );
    assert_eq!(
        FileConflict::Lines.to_string(),
        "the same lines changed differently on both sides"
    );
    assert_eq!(
        FileConflict::Binary.to_string(),
        "a binary file changed differently on both sides"
    );
    assert_eq!(
        FileConflict::Kind.to_string(),
        "the file kind set differently on both sides"
    );
}

#[test]
fn a_name_error_and_a_path_error_say_what_is_wrong_with_the_text() {
    assert_eq!(
        NameError::Length(0).to_string(),
        "a name is 1 to 63 bytes, not 0"
    );
    assert_eq!(
        NameError::LeadingHyphen.to_string(),
        "a name starts with a letter or a digit"
    );
    assert_eq!(
        PathError::Empty.to_string(),
        "a path has at least one component"
    );
    assert!(
        PathError::InvalidComponent
            .to_string()
            .starts_with("a component is non-empty")
    );
}

#[test]
fn a_principal_error_says_which_part_is_wrong_and_the_parts_are_given_back() {
    assert_eq!(
        PrincipalError::Issuer.to_string(),
        "an issuer is not empty and holds no whitespace"
    );
    assert_eq!(
        PrincipalError::Subject.to_string(),
        "a subject is not empty"
    );
    let principal = Principal::new("https://id", "ada").unwrap();
    assert_eq!(principal.issuer(), "https://id");
    assert_eq!(principal.subject(), "ada");
}

#[test]
fn a_tree_error_names_the_path_or_the_tree() {
    let address = Address::of(b"tree");
    assert_eq!(
        DeriveError::MissingTree(address).to_string(),
        format!("tree {address} was not supplied")
    );
    assert_eq!(
        DeriveError::NotFound(path("a")).to_string(),
        "a holds nothing to delete"
    );
    assert_eq!(
        DeriveError::NotADirectory(path("a")).to_string(),
        "a is a file, not a directory"
    );
    assert_eq!(
        DeriveError::IsADirectory(path("a")).to_string(),
        "a is a non-empty directory"
    );
}

#[test]
fn an_error_after_a_landing_is_its_cause_and_says_what_its_cause_says() {
    let queue = AfterLandingError::Queue(QueueChecksError::NoRepository(RepositoryId::new(3)));
    assert_eq!(queue.to_string(), "no repository 3");
    assert_eq!(queue.source().unwrap().to_string(), "no repository 3");

    let cause = RevertError::Land(LandError::BaseNotOnMain);
    let revert = AfterLandingError::Revert(cause.clone());
    assert_eq!(revert.to_string(), cause.to_string());
    assert_eq!(revert.source().unwrap().to_string(), cause.to_string());
}
