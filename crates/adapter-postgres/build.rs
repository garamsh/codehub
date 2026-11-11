//! Rebuilds the crate when a migration is added, since the migrations are embedded at compile time.

fn main() {
    println!("cargo::rerun-if-changed=migrations");
}
