//! The encodings of the object kinds: decoding gives back the value, the address is the SHA-256 of
//! the encoding, and bytes that are not an encoding are refused without a panic.

pub mod generators;

use app::domain::address::Address;
use app::domain::changeset::Changeset;
use app::domain::file_version::FileVersionNode;
use app::domain::tree::{Entry, Tree};
use proptest::collection::{btree_map, vec};
use proptest::prelude::*;
use sha2::{Digest, Sha256};

use generators::{address, changeset, narrow_file, wide_component, wide_file, wide_path};

fn tree() -> impl Strategy<Value = Tree> {
    let entry = prop_oneof![
        wide_file().prop_map(Entry::File),
        address().prop_map(Entry::Directory)
    ];
    btree_map(wide_component(), entry, 0..5).prop_map(|entries| Tree { entries })
}

fn file_version_node() -> impl Strategy<Value = FileVersionNode> {
    (address(), wide_path(), narrow_file(), vec(address(), 0..3)).prop_map(
        |(changeset, path, file, previous)| FileVersionNode {
            changeset,
            path,
            file,
            previous,
        },
    )
}

/// The input with one byte changed, or cut short, or with bytes added.
fn damaged(bytes: Vec<u8>) -> impl Strategy<Value = Vec<u8>> {
    let length = bytes.len();
    let flipped = bytes.clone();
    let cut = bytes.clone();
    prop_oneof![
        (0..length, 1_u8..=255).prop_map(move |(at, xor)| {
            let mut copy = flipped.clone();
            if let Some(byte) = copy.get_mut(at) {
                *byte ^= xor;
            }
            copy
        }),
        (0..length).prop_map(move |keep| cut.get(..keep).unwrap_or_default().to_vec()),
        vec(any::<u8>(), 1..4).prop_map(move |extra| [bytes.clone(), extra].concat()),
    ]
}

proptest! {
    #[test]
    fn a_changeset_decodes_to_the_value_it_encodes(value in changeset()) {
        let bytes = value.encode().unwrap();
        prop_assert_eq!(Changeset::decode(&bytes), Ok(value));
    }

    #[test]
    fn a_tree_decodes_to_the_value_it_encodes(value in tree()) {
        let bytes = value.encode().unwrap();
        prop_assert_eq!(Tree::decode(&bytes), Ok(value));
    }

    #[test]
    fn a_file_version_node_decodes_to_the_value_it_encodes(value in file_version_node()) {
        let bytes = value.encode().unwrap();
        prop_assert_eq!(FileVersionNode::decode(&bytes), Ok(value));
    }

    #[test]
    fn an_encoding_is_deterministic_and_its_address_is_its_sha256(value in changeset()) {
        let bytes = value.encode().unwrap();
        prop_assert_eq!(&value.clone().encode().unwrap(), &bytes);
        let digest: [u8; 32] = Sha256::digest(&bytes).into();
        prop_assert_eq!(value.id().unwrap(), Address::from_digest(digest));
    }

    #[test]
    fn a_tree_and_a_node_are_addressed_by_the_sha256_of_their_encoding(
        tree in tree(),
        node in file_version_node(),
    ) {
        let digest: [u8; 32] = Sha256::digest(tree.encode().unwrap()).into();
        prop_assert_eq!(tree.address().unwrap(), Address::from_digest(digest));
        let digest: [u8; 32] = Sha256::digest(node.encode().unwrap()).into();
        prop_assert_eq!(node.address().unwrap(), Address::from_digest(digest));
    }

    #[test]
    fn values_that_differ_have_addresses_that_differ(a in changeset(), b in changeset()) {
        prop_assert_eq!(a == b, a.id().unwrap() == b.id().unwrap());
    }

    #[test]
    fn a_blob_is_addressed_by_the_sha256_of_its_content(content in vec(any::<u8>(), 0..64)) {
        let digest: [u8; 32] = Sha256::digest(&content).into();
        prop_assert_eq!(Address::of(&content), Address::from_digest(digest));
    }

    #[test]
    fn arbitrary_bytes_are_refused_or_are_the_encoding_of_what_they_decode_to(
        bytes in vec(any::<u8>(), 0..96),
    ) {
        if let Ok(value) = Changeset::decode(&bytes) {
            prop_assert_eq!(value.encode().unwrap(), bytes.clone());
        }
        if let Ok(value) = Tree::decode(&bytes) {
            prop_assert_eq!(value.encode().unwrap(), bytes.clone());
        }
        if let Ok(value) = FileVersionNode::decode(&bytes) {
            prop_assert_eq!(value.encode().unwrap(), bytes);
        }
    }

    #[test]
    fn a_damaged_encoding_is_refused_or_is_the_encoding_of_what_it_decodes_to(
        (value, input) in changeset().prop_flat_map(|value| {
            let bytes = value.encode().unwrap();
            (Just(value), damaged(bytes))
        }),
    ) {
        match Changeset::decode(&input) {
            Ok(decoded) => {
                prop_assert_eq!(decoded.encode().unwrap(), input);
            }
            Err(_) => {
                prop_assert_ne!(value.encode().unwrap(), input);
            }
        }
    }

    #[test]
    fn a_cut_or_extended_encoding_is_refused(
        (value, cut, extra) in changeset().prop_flat_map(|value| {
            let length = value.encode().unwrap().len();
            (Just(value), 0..length, vec(any::<u8>(), 1..4))
        }),
    ) {
        let bytes = value.encode().unwrap();
        prop_assert!(Changeset::decode(bytes.get(..cut).unwrap()).is_err());
        prop_assert!(Changeset::decode(&[bytes, extra].concat()).is_err());
    }

    #[test]
    fn a_damaged_tree_or_node_is_refused_or_is_the_encoding_of_what_it_decodes_to(
        (tree_input, node_input) in (tree(), file_version_node()).prop_flat_map(|(tree, node)| {
            (damaged(tree.encode().unwrap()), damaged(node.encode().unwrap()))
        }),
    ) {
        if let Ok(decoded) = Tree::decode(&tree_input) {
            prop_assert_eq!(decoded.encode().unwrap(), tree_input);
        }
        if let Ok(decoded) = FileVersionNode::decode(&node_input) {
            prop_assert_eq!(decoded.encode().unwrap(), node_input);
        }
    }
}
