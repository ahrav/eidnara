use kernel::{
    AdmissionFacts, ClaimFacts, ClaimStanding, Disposition, ObjectRow, ServedStanding,
    SurfaceVisibility,
};

use crate::canonical_memory::MEMORY_DOMAIN_ID;

/// `scope_names_project` is the caller's
/// [`kernel::ProjectScope::names_project`] verdict on the decision's scope.
pub fn eligible(facts: &ClaimFacts, scope_names_project: bool) -> bool {
    standing_eligible(
        &facts.object,
        &facts.served,
        facts.own_admission.as_ref(),
        scope_names_project,
    )
}

/// [`eligible`] over a [`ClaimStanding`], which carries every fact the predicate reads.
pub fn standing_is_eligible(standing: &ClaimStanding, scope_names_project: bool) -> bool {
    standing_eligible(
        &standing.object,
        &standing.served,
        standing.own_admission.as_ref(),
        scope_names_project,
    )
}

fn standing_eligible(
    object: &ObjectRow,
    served: &ServedStanding,
    own_admission: Option<&AdmissionFacts>,
    scope_names_project: bool,
) -> bool {
    let ServedStanding::Served(served) = served else {
        return false;
    };
    object.domain_id == MEMORY_DOMAIN_ID
        && scope_names_project
        && served.explicit_search != SurfaceVisibility::Hidden
        && object.superseded_by.is_none()
        && own_admission.is_some_and(|own| {
            !matches!(
                own.disposition,
                Disposition::Superseded
                    | Disposition::Contradicted
                    | Disposition::Rejected
                    | Disposition::Quarantined
            )
        })
}
