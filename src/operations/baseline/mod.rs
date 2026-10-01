//! Installing a signed baseline into a workspace: the whole set, or nothing.
//!
//! [`crate::baseline`] answers "is this exactly the set someone signed". This
//! module answers the next question — "what does it take to put that set on
//! disk without ever leaving half of it there".
//!
//! A [`crate::baseline::VerifiedBaseline`] can only be built by `bind`, which is not
//! incremental, so by the time anything here runs every script's bytes have
//! already been checked against the signed manifest. What is left is the
//! filesystem, where all-or-nothing is not free: `N` scripts are `N` renames,
//! and the fourth can fail. Every write is therefore staged and undoable, and
//! one failure walks the successful ones back before returning.
//!
//! **Provenance is recorded for the set, not per script.** The Battery record
//! was considered and rejected on two counts. Four of its seven fields — the
//! git URL, the requested ref, the resolved commit, the source path — have no
//! meaning for a baseline and could only be filled with something untrue. More
//! seriously, `battery::installing_battery` scans that directory to answer gate
//! E of the Remote Cue plane: a baseline script recorded there would read as
//! installed *by a battery*, and any node whose `trust.remote_cue_batteries`
//! named that battery would silently have made it remotely runnable. Sharing
//! the file would have widened an authorization decision as a side effect of
//! reusing a struct.

mod install;
mod publish;
mod record;

fn map_baseline_error(error: crate::baseline::BaselineError) -> crate::operations::OperationError {
    crate::operations::OperationError::new(
        crate::operations::OperationErrorCode::InvalidInput,
        error.to_string(),
    )
}

pub use install::{install_baseline, rollback_baseline};
pub use publish::{bodies_for_manifest, publish_baseline, PublishedBaseline};
pub use record::{
    installed_baseline, installed_baseline_path, observed_baseline_id, retained_current_path,
    retained_previous, retained_previous_path, InstalledBaseline, RetainedBaseline,
    BASELINE_SCRIPT_MODE,
};

#[cfg(test)]
mod tests;
