//! `clean [--all]` (plan, rows 41, 53), the last step of a task. `clean` takes every VM and every
//! snapshot of the namespace, kept and cached ones too, before the images, then the data of every
//! session; the journals, the runtime of microsandbox and `stand-guest` stay, so the next seal
//! shows `cleaned`. `clean --all` deletes the whole folder but the lock and a `stand.toml` in it.
//! Either way the snapshot ref of the work tree it runs in goes, and one line says what went:
//! `cleaned 1 session, 3 snapshots`.

use sandbox::Sandbox;
use store::Clean;

use crate::error::StandError;
use crate::{Env, Exit, lock, policy, stand_file, sweep};

pub(crate) fn run(
    what: Clean,
    sandbox: &impl Sandbox,
    env: &mut Env<'_>,
) -> Result<Exit, StandError> {
    // Outside a work tree there is no ref to drop and no stand file to wait by.
    let worktree = source::open(env.cwd).ok();
    let stand = worktree.as_ref().and_then(|worktree| {
        let (policy, _) = policy(env.store).ok()?;
        spec::load(&stand_file(env.cwd, worktree), &policy).ok()
    });
    let _lock = lock::wait(
        env.store,
        lock::limit(stand.as_ref()),
        env.cancel,
        &mut *env.err,
    )?;
    let left = match what {
        Clean::Sessions => {
            let left = sandbox.leftovers()?;
            sweep::clear(sandbox, &left)?;
            sandbox.prune_images(&[])?;
            Some(left)
        }
        // Without the adapter: a microsandbox home that no longer opens is wiped too, and this
        // process holds no file of the SDK's database open, which Windows would refuse to delete.
        // A VM a crashed run left holds its files until its `max_duration`; then run it again.
        Clean::All => None,
    };
    let sessions = sweep::count(env.store.clean(what)?, "session", "sessions");
    if let Some(worktree) = &worktree {
        source::forget(worktree)?;
    }
    // The images it prunes go uncounted: the port lists none.
    let text = match left {
        Some(left) => {
            let mut parts = vec![sessions];
            if !left.sandboxes.is_empty() {
                parts.push(sweep::count(left.sandboxes.len(), "VM", "VMs"));
            }
            parts.push(sweep::count(left.snapshots.len(), "snapshot", "snapshots"));
            format!("cleaned {}", parts.join(", "))
        }
        None => format!("cleaned {sessions} and the rest of the stand folder"),
    };
    writeln!(env.out, "{text}").map_err(StandError::io("stdout"))?;
    Ok(Exit::Passed)
}
