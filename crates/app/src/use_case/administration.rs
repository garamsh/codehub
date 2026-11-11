//! Who administers an organisation's members (ADR 0030).

/// Whether codehub or an outside layer administers members, passed by the composition site.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Administration {
    /// codehub keeps members and grants: the members operations work, and creating an
    /// organisation makes its creator an admin.
    BuiltIn,
    /// An outside layer decides and keeps members: the members operations are refused, creating
    /// an organisation writes no admin grant, and an organisation is created on first use where
    /// the authorisation decision allows `CreateOrganisation`.
    External,
}
