use std::path::PathBuf;

use clap::{Args, Parser};

#[derive(Debug, Parser)]
#[command(
    name = "stop-slop",
    version,
    about = "Checks the TypeScript files of a branch against personal conventions."
)]
pub(crate) enum Command {
    /// Checks the files of the branch.
    Check(CheckArgs),
    /// Lists the rules.
    Rules {
        /// Prints JSON with the message, the agent hint and whether the rule is active here.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Args)]
pub(crate) struct CheckArgs {
    /// The base branch or commit; searched when not given.
    #[arg(long, conflicts_with_all = ["files", "all"])]
    pub(crate) base: Option<String>,
    /// Reads the files from git at this revision instead of the disk.
    #[arg(long, requires = "base", conflicts_with = "fix")]
    pub(crate) rev: Option<String>,
    /// Checks only these files.
    #[arg(long, num_args = 1.., conflicts_with = "all")]
    pub(crate) files: Vec<PathBuf>,
    /// Checks every file of the repository.
    #[arg(long)]
    pub(crate) all: bool,
    /// Applies the fixes of the rules that have one, then checks the files again.
    #[arg(long)]
    pub(crate) fix: bool,
    /// Prints JSON for an agent.
    #[arg(long)]
    pub(crate) json: bool,
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::Command;

    #[test]
    fn flags_are_consistent() {
        Command::command().debug_assert();
    }
}
