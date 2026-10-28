//! What needs every service at once: kinds from `depends_on` and its rules, the levels, and the
//! prepare service of the version disk with what restoring its snapshot asks of the others.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, BTreeSet};

use crate::tree::Field;
use crate::{Condition, Invalid, Mount, Service, ServiceKind, ServiceName};

pub(crate) struct Graph {
    pub(crate) levels: Vec<Vec<ServiceName>>,
    pub(crate) prepare: Option<ServiceName>,
}

/// Sets the `kind` of every service.
pub(crate) fn of(
    root: &Field,
    services: &mut BTreeMap<ServiceName, Service>,
    workload: &ServiceName,
) -> Result<Graph, Invalid> {
    let setup = waits(root, services, workload)?;
    for (name, service) in services.iter_mut() {
        service.kind = if name == workload {
            ServiceKind::Workload
        } else if setup.contains(name) {
            ServiceKind::Setup
        } else {
            ServiceKind::LongRunning
        };
    }
    let levels = levels(root, services)?;
    let prepare = prepare(root, services, &levels)?;
    Ok(Graph { levels, prepare })
}

/// Checks every `depends_on` entry and gives the setup services: those waited for with
/// `service_completed_successfully`.
fn waits(
    root: &Field,
    services: &BTreeMap<ServiceName, Service>,
    workload: &ServiceName,
) -> Result<BTreeSet<ServiceName>, Invalid> {
    let mut waited: BTreeMap<&ServiceName, (&ServiceName, Condition)> = BTreeMap::new();
    for (name, service) in services {
        for (dependency, &condition) in &service.depends_on {
            let at = service_field(root, name.as_str())
                .at("depends_on")
                .at(dependency.as_str());
            let Some(target) = services.get(dependency) else {
                return Err(at.invalid(format!("no service `{}`", dependency.as_str())));
            };
            if dependency == workload {
                return Err(at.invalid(format!(
                    "nothing waits for the workload `{}`: a repeat ends when it exits",
                    workload.as_str()
                )));
            }
            if condition == Condition::Healthy && target.healthcheck.is_none() {
                return Err(at.invalid(format!(
                    "`{}` has no `healthcheck` to be healthy by",
                    dependency.as_str()
                )));
            }
            match waited.entry(dependency) {
                Entry::Vacant(entry) => {
                    entry.insert((name, condition));
                }
                Entry::Occupied(entry) if entry.get().1 != condition => {
                    let (first, other) = *entry.get();
                    return Err(at.invalid(format!(
                        "`{}` waits for `{}` until {}, and `{}` until {}: a service is either \
                         setup or long-running, so all wait for it the same way",
                        name.as_str(),
                        dependency.as_str(),
                        condition_name(condition),
                        first.as_str(),
                        condition_name(other),
                    )));
                }
                Entry::Occupied(_) => {}
            }
        }
    }
    Ok(waited
        .into_iter()
        .filter(|(_, (_, condition))| *condition == Condition::CompletedSuccessfully)
        .map(|(name, _)| name.clone())
        .collect())
}

fn condition_name(condition: Condition) -> &'static str {
    match condition {
        Condition::Started => "service_started",
        Condition::Healthy => "service_healthy",
        Condition::CompletedSuccessfully => "service_completed_successfully",
    }
}

/// A service is on the level after the last level of what it depends on.
fn levels(
    root: &Field,
    services: &BTreeMap<ServiceName, Service>,
) -> Result<Vec<Vec<ServiceName>>, Invalid> {
    let mut placed = BTreeSet::new();
    let mut levels: Vec<Vec<ServiceName>> = Vec::new();
    while placed.len() < services.len() {
        let level: Vec<ServiceName> = services
            .iter()
            .filter(|(name, service)| {
                !placed.contains(*name)
                    && service.depends_on.keys().all(|name| placed.contains(name))
            })
            .map(|(name, _)| name.clone())
            .collect();
        if level.is_empty() {
            return Err(cycle(root, services, &placed));
        }
        placed.extend(level.iter().cloned());
        levels.push(level);
    }
    Ok(levels)
}

/// Every service not `placed` waits for another one not placed, so following those finds a cycle.
fn cycle(
    root: &Field,
    services: &BTreeMap<ServiceName, Service>,
    placed: &BTreeSet<ServiceName>,
) -> Invalid {
    let left = |name: &&ServiceName| !placed.contains(*name);
    let mut path: Vec<&ServiceName> = Vec::new();
    let mut next = services.keys().find(left);
    while let Some(name) = next {
        if let Some(start) = path.iter().position(|seen| *seen == name) {
            path.drain(..start);
            break;
        }
        path.push(name);
        next = services[name].depends_on.keys().find(left);
    }
    let names: Vec<String> = path
        .iter()
        .map(|name| format!("`{}`", name.as_str()))
        .collect();
    let message = match &names[..] {
        [one] => format!("{one} waits for itself"),
        [first @ .., last] => format!("{} and {last} wait for each other", first.join(", ")),
        [] => "the services wait for each other".to_owned(),
    };
    let first = path.first().map_or("", |name| name.as_str());
    service_field(root, first).at("depends_on").invalid(message)
}

/// The prepare service is a setup service with `${STAND_REPO}` that every other service with
/// `${STAND_REPO}` waits for. Only the first such setup service in `levels` can be it: one that all
/// the others wait for comes before them.
fn prepare(
    root: &Field,
    services: &BTreeMap<ServiceName, Service>,
    levels: &[Vec<ServiceName>],
) -> Result<Option<ServiceName>, Invalid> {
    let on_tree = |service: &Service| {
        service
            .mounts
            .iter()
            .any(|mount| matches!(mount, Mount::Tree))
    };
    let Some(prepare) = levels
        .iter()
        .flatten()
        .find(|name| services[*name].kind == ServiceKind::Setup && on_tree(&services[*name]))
    else {
        return Ok(None);
    };
    let others = || {
        services
            .iter()
            .filter(|(name, service)| *name != prepare && on_tree(service))
    };
    for (name, _) in others() {
        if !waits_for(services, name, prepare) {
            return Err(service_field(root, name.as_str()).invalid(format!(
                "`{}` mounts `${{STAND_REPO}}` and does not wait for `{}`, the setup service \
                 that prepares the tree: every other `${{STAND_REPO}}` service starts from its \
                 snapshot",
                name.as_str(),
                prepare.as_str()
            )));
        }
    }
    let image = &services[prepare].image;
    for (name, service) in others() {
        if service.image != *image {
            return Err(service_field(root, name.as_str())
                .at("image")
                .invalid(format!(
                    "`{}` is not `{image}` of the prepare service `{}`: every `${{STAND_REPO}}` \
                 service starts from its snapshot, which keeps the image",
                    service.image,
                    prepare.as_str()
                )));
        }
    }
    let harness = services[prepare]
        .mounts
        .iter()
        .position(|mount| matches!(mount, Mount::Harness { .. }));
    if let Some(index) = harness {
        return Err(service_field(root, prepare.as_str())
            .at("volumes")
            .item(index)
            .invalid(format!(
                "the prepare service `{}` binds only `${{STAND_REPO}}`: other services start from \
                 its snapshot, and its restore needs every folder it bound",
                prepare.as_str()
            )));
    }
    Ok(Some(prepare.clone()))
}

/// `from` depends on `to`, directly or through others.
fn waits_for(
    services: &BTreeMap<ServiceName, Service>,
    from: &ServiceName,
    to: &ServiceName,
) -> bool {
    let mut seen = BTreeSet::new();
    let mut next = vec![from];
    while let Some(name) = next.pop() {
        for dependency in services[name].depends_on.keys() {
            if dependency == to {
                return true;
            }
            if seen.insert(dependency) {
                next.push(dependency);
            }
        }
    }
    false
}

/// The services that run in setup: the setup services and what they wait for, directly or
/// through others.
pub(crate) fn in_setup(services: &BTreeMap<ServiceName, Service>) -> BTreeSet<&ServiceName> {
    let mut found = BTreeSet::new();
    let mut next: Vec<&ServiceName> = services
        .iter()
        .filter(|(_, service)| service.kind == ServiceKind::Setup)
        .map(|(name, _)| name)
        .collect();
    while let Some(name) = next.pop() {
        if found.insert(name) {
            next.extend(services[name].depends_on.keys());
        }
    }
    found
}

fn service_field<'a>(root: &Field<'a>, name: &str) -> Field<'a> {
    root.at("services").at(name)
}
