//! The source-control driving port, served by the landing and history use cases over the driven
//! ports it is given.

use crate::domain::address::Address;
use crate::domain::file_version::FileVersionNode;
use crate::domain::path::Path;
use crate::port::driven::changeset_log::ChangesetLog;
use crate::port::driven::derived_data::DerivedData;
use crate::port::driven::draft_store::DraftStore;
use crate::port::driven::object_store::ObjectStore;
use crate::port::driving::source_control::{SourceControl, SourceControlError};
use crate::use_case::history::history;
use crate::use_case::land::land;

/// Source control over the driven ports the composition site passes to it.
pub struct SourceControlService<'a, O, L, D, S> {
    objects: &'a O,
    log: &'a L,
    derived: &'a D,
    drafts: &'a S,
}

impl<'a, O, L, D, S> SourceControlService<'a, O, L, D, S> {
    /// Source control that stores blobs in `objects`, changesets and `main` in `log`, derived
    /// data in `derived`, and reads drafts from `drafts`.
    pub const fn new(objects: &'a O, log: &'a L, derived: &'a D, drafts: &'a S) -> Self {
        Self {
            objects,
            log,
            derived,
            drafts,
        }
    }
}

impl<O, L, D, S> SourceControl for SourceControlService<'_, O, L, D, S>
where
    O: ObjectStore + Sync,
    L: ChangesetLog + Sync,
    D: DerivedData + Sync,
    S: DraftStore + Sync,
{
    async fn land(&self, draft: Address) -> Result<Address, SourceControlError> {
        Ok(land(self.objects, self.log, self.derived, self.drafts, draft).await?)
    }

    async fn history(
        &self,
        at: Address,
        path: &Path,
    ) -> Result<Vec<FileVersionNode>, SourceControlError> {
        Ok(history(self.log, self.derived, at, path).await?)
    }
}
