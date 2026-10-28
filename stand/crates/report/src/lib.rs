//! The raw outputs of a stand run into `report.json`, the `--json` line and the summary. `run`
//! returns `Raw`, so this crate sits below `run` and starts with those types (plan, row 85).

mod raw;

pub use raw::{
    BeforeOutcome, Cause, CoverageOutcome, Extra, Failure, FaultOutcome, FaultRepeat,
    InstanceOutput, Pass, PassPair, Phase, ProbeRan, ProbeRule, ProfileOutcome, Raw, RepeatOutput,
    Role, SecondPassOutcome, Setup, SetupSide,
};
