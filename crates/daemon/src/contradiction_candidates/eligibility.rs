use kernel::{ClaimFacts, Disposition, ServedStanding, SurfaceVisibility};

use crate::canonical_memory::MEMORY_DOMAIN_ID;

/// `scope_names_project` is the caller's
/// [`kernel::ProjectScope::names_project`] verdict on the decision's scope.
pub fn eligible(facts: &ClaimFacts, scope_names_project: bool) -> bool {
    let ServedStanding::Served(served) = &facts.served else {
        return false;
    };
    facts.object.domain_id == MEMORY_DOMAIN_ID
        && scope_names_project
        && served.explicit_search != SurfaceVisibility::Hidden
        && facts.object.superseded_by.is_none()
        && facts.own_admission.as_ref().is_some_and(|own| {
            !matches!(
                own.disposition,
                Disposition::Superseded
                    | Disposition::Contradicted
                    | Disposition::Rejected
                    | Disposition::Quarantined
            )
        })
}
