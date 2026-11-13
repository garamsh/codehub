//! The check that `docs/deployment.md` names every setting of a binary and no other.

use std::collections::BTreeSet;

use clap::Command;

/// The names in `text` that start with `--` or `CODEHUB_`, each read up to the first character a
/// name does not hold. A bare prefix names nothing.
fn names(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    for start in text
        .match_indices("--")
        .map(|(at, _)| at)
        .chain(text.match_indices("CODEHUB_").map(|(at, _)| at))
    {
        let name: String = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
            .collect();
        // A bare prefix, or a run of dashes such as a table's rule, names no setting.
        if !name.trim_start_matches('-').is_empty() && name != "CODEHUB_" {
            found.insert(name);
        }
    }
    found
}

/// Asserts that the section `heading` of `docs/deployment.md` holds one table row for each option
/// of `command`, that no other `--option` or `CODEHUB_*` variable appears in the section, and that
/// every option whose value is hidden in `--help` is marked a secret.
///
/// A row is `| option | variable | secret | purpose |`, with `-` where an option has no variable.
///
/// # Panics
///
/// Panics where the contract and the command differ, naming the difference.
pub fn assert_contract(command: &Command, heading: &str) {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/deployment.md");
    let doc = std::fs::read_to_string(path).unwrap();
    let marker = format!("## {heading}");
    let section: String = doc
        .lines()
        .skip_while(|line| *line != marker)
        .skip(1)
        .take_while(|line| !line.starts_with("## "))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !section.is_empty(),
        "docs/deployment.md has no section `{heading}`"
    );

    let rows: Vec<Vec<&str>> = section
        .lines()
        .filter(|line| line.starts_with('|'))
        .map(|line| line.trim_matches('|').split('|').map(str::trim).collect())
        .filter(|cells: &Vec<&str>| cells.first().is_some_and(|cell| cell.starts_with("`--")))
        .collect();

    let mut listed = BTreeSet::new();
    for arg in command
        .get_arguments()
        .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "version"))
    {
        let long = format!("--{}", arg.get_long().unwrap());
        let variable = arg.get_env().map(|env| env.to_str().unwrap().to_owned());
        let row = rows
            .iter()
            .find(|cells| cells[0] == format!("`{long}`"))
            .unwrap_or_else(|| panic!("`{long}` is not in the `{heading}` table"));
        let expected = variable
            .as_ref()
            .map_or_else(|| "-".to_owned(), |name| format!("`{name}`"));
        assert_eq!(row[1], expected, "the variable of `{long}`");
        if arg.is_hide_env_values_set() {
            assert!(
                row[2].starts_with("yes"),
                "`{long}` hides its value, so it is a secret"
            );
        }
        listed.insert(long);
        listed.extend(variable);
    }
    let unknown: Vec<_> = names(&section).difference(&listed).cloned().collect();
    assert!(
        unknown.is_empty(),
        "`{heading}` names what the command does not have: {unknown:?}"
    );
}
