//! The wire types of `ch` against the schemas of `crates/adapter-http/openapi.json`.

use std::collections::BTreeSet;

use ch::wire::{
    Access, Created, File, Grant, Header, History, Kind, Landed, Landing, Member, NewName,
    Principal, Problem, Role, StagedFile, TransferGrant, TransferMethod, Upload, Version,
};
use serde::Serialize;
use serde_json::Value;

#[cfg(test)]
fn schemas() -> Value {
    let path = format!(
        "{}/../adapter-http/openapi.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let text = std::fs::read_to_string(&path).unwrap();
    let document: Value = serde_json::from_str(&text).unwrap();
    document["components"]["schemas"].clone()
}

#[cfg(test)]
fn keys(object: &serde_json::Map<String, Value>) -> BTreeSet<String> {
    object.keys().cloned().collect()
}

/// Asserts that `value` serialises to exactly the properties of `schema`.
#[cfg(test)]
fn assert_fields<T: Serialize>(value: &T, schema: &Value, name: &str) {
    let serialised = serde_json::to_value(value).unwrap();
    let properties = schema["properties"].as_object().unwrap();
    assert_eq!(
        keys(serialised.as_object().unwrap()),
        keys(properties),
        "{name}"
    );
}

#[cfg(test)]
fn address() -> String {
    format!("sha256:{}", "0".repeat(64))
}

#[cfg(test)]
fn grant() -> TransferGrant {
    TransferGrant {
        url: "http://s".to_owned(),
        method: TransferMethod::Put,
        headers: vec![Header {
            name: "n".to_owned(),
            value: "v".to_owned(),
        }],
        expires_at: 1,
    }
}

#[test]
fn has_the_fields_of_every_schema() {
    let schemas = schemas();
    let who = || ("https://id.test".to_owned(), "ada".to_owned());
    let (issuer, subject) = who();

    assert_fields(
        &NewName { name: "a".into() },
        &schemas["NewName"],
        "NewName",
    );
    assert_fields(
        &Created { name: "a".into() },
        &schemas["Created"],
        "Created",
    );
    assert_fields(
        &Principal {
            issuer: issuer.clone(),
            subject: subject.clone(),
        },
        &schemas["Principal"],
        "Principal",
    );
    assert_fields(
        &Member {
            issuer: issuer.clone(),
            subject: subject.clone(),
            role: Role::Admin,
        },
        &schemas["Member"],
        "Member",
    );
    assert_fields(
        &Grant {
            issuer,
            subject,
            prefix: String::new(),
            access: Access::Write,
        },
        &schemas["Grant"],
        "Grant",
    );
    assert_fields(
        &Upload {
            address: address(),
            size: 1,
        },
        &schemas["Upload"],
        "Upload",
    );
    assert_fields(
        &StagedFile {
            kind: Kind::Regular,
            content: address(),
        },
        &schemas["StagedFile"],
        "StagedFile",
    );
    assert_fields(
        &Landing {
            message: "m".into(),
        },
        &schemas["Landing"],
        "Landing",
    );
    assert_fields(
        &Landed {
            changeset: address(),
        },
        &schemas["Landed"],
        "Landed",
    );
    assert_fields(
        &File {
            kind: Kind::Regular,
            download: grant(),
        },
        &schemas["File"],
        "File",
    );
    assert_fields(
        &Version {
            changeset: address(),
            path: "a".into(),
            kind: Kind::Regular,
            content: address(),
            previous: vec![],
        },
        &schemas["Version"],
        "Version",
    );
    assert_fields(
        &History { versions: vec![] },
        &schemas["History"],
        "History",
    );
    assert_fields(
        &Problem {
            kind: "about:blank".into(),
            title: "t".into(),
            status: 400,
            detail: "d".into(),
            paths: vec![],
        },
        &schemas["Problem"],
        "Problem",
    );
}

#[test]
fn has_the_fields_of_a_transfer_grant_and_its_headers() {
    let schemas = schemas();
    let schema = &schemas["TransferGrant"];
    assert_fields(&grant(), schema, "TransferGrant");
    let header = &schema["properties"]["headers"]["items"];
    assert_fields(&grant().headers[0], header, "Header");
}

#[cfg(test)]
fn allowed(schema: &Value) -> BTreeSet<String> {
    schema["enum"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect()
}

#[cfg(test)]
fn written<T: Serialize>(values: &[T]) -> BTreeSet<String> {
    values
        .iter()
        .map(|value| {
            serde_json::to_value(value)
                .unwrap()
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}

#[test]
fn has_the_values_of_the_enumerations() {
    let schemas = schemas();
    let kinds = [Kind::Regular, Kind::Executable];
    assert_eq!(allowed(&schemas["Kind"]), written(&kinds));
    let roles = [Role::Admin, Role::Member];
    let role = &schemas["Member"]["properties"]["role"];
    assert_eq!(allowed(role), written(&roles));
    let accesses = [Access::Read, Access::Write];
    let access = &schemas["Grant"]["properties"]["access"];
    assert_eq!(allowed(access), written(&accesses));
    let methods = [TransferMethod::Get, TransferMethod::Put];
    let method = &schemas["TransferGrant"]["properties"]["method"];
    assert_eq!(allowed(method), written(&methods));
}
