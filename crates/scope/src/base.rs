use repo::{BranchDistance, Repo, Snapshot};

use crate::{Base, BaseReason, PrBase, ScopeError};

pub(crate) fn find(
    repo: &Repo,
    snapshot: &Snapshot,
    pr: &dyn PrBase,
    flag: Option<String>,
) -> Result<Base, ScopeError> {
    if let Some(reference) = flag {
        let Some(oid) = repo.resolve(&reference)? else {
            return Err(ScopeError::UnknownBase(reference));
        };
        return Ok(Base {
            reference,
            oid,
            reason: BaseReason::Flag,
        });
    }
    if matches!(snapshot, Snapshot::Rev(_)) {
        return Err(ScopeError::RevWithoutBase);
    }
    let (distances, current) =
        crate::both(|| snapshot.branch_distances(), || repo.current_branch());
    let (distances, current) = (distances?, current?);
    let current = current.as_deref();
    // `pr` is asked on this thread, after the join: `PrBase` is not `Sync`, and the question
    // waits for `origin/<branch>` in `distances` anyway.
    if let Some(base) = current
        .and_then(|branch| of_pr(repo, pr, &distances, branch))
        .or_else(|| ancestor(&distances, current))
    {
        return Ok(base);
    }
    let tip = repo.default_branch()?.ok_or(ScopeError::NoBase)?;
    Ok(Base {
        reference: tip.name,
        oid: tip.oid,
        reason: BaseReason::Default,
    })
}

/// Asks only when `origin/<branch>` exists: a branch that was never pushed has no pull
/// request, and `gh` costs half a second of network.
fn of_pr(repo: &Repo, pr: &dyn PrBase, distances: &[BranchDistance], branch: &str) -> Option<Base> {
    find_tip(distances, &format!("origin/{branch}"))?;
    let name = pr.base_of(repo.root(), branch)?;
    // `origin/<name>` first: the skill fetches before `check`, a local branch may be old.
    let found =
        find_tip(distances, &format!("origin/{name}")).or_else(|| find_tip(distances, &name))?;
    Some(base(found, BaseReason::Pr))
}

/// The branch whose common commit with the head is nearest to it. `behind` is 0 for the
/// current branch, its descendants and branches at the head, so they drop out.
/// `origin/<current>` is left out by name: when the last commits are not pushed yet it is
/// behind the head and would be the nearest. On a tie in `behind` the smaller `ahead` wins,
/// then the first in ref order.
fn ancestor(distances: &[BranchDistance], current: Option<&str>) -> Option<Base> {
    let own = current.map(|branch| format!("origin/{branch}"));
    let found = distances
        .iter()
        .filter(|distance| distance.behind > 0 && Some(&distance.tip.name) != own.as_ref())
        .min_by_key(|distance| (distance.behind, distance.ahead))?;
    Some(base(found, BaseReason::Ancestor))
}

fn base(distance: &BranchDistance, reason: BaseReason) -> Base {
    Base {
        reference: distance.tip.name.clone(),
        oid: distance.tip.oid.clone(),
        reason,
    }
}

fn find_tip<'a>(distances: &'a [BranchDistance], name: &str) -> Option<&'a BranchDistance> {
    distances.iter().find(|distance| distance.tip.name == name)
}
