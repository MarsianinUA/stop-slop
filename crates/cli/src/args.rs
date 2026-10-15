use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    name = "stop-slop",
    version,
    about = "Checks the TypeScript files of a branch against personal conventions."
)]
pub(crate) enum Command {
    /// Lists the rules.
    Rules {
        /// Prints JSON with the message, the agent hint and whether the rule is active here.
        #[arg(long)]
        json: bool,
    },
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
