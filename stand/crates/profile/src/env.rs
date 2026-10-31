use crate::{NodeEnv, ProfileMode};

/// `inherited` is the service's own `NODE_OPTIONS`; it goes after
/// `--require <preload_dir>/preload.cjs`, so the profile also covers what its own `--require`
/// runs. `profile_dir` is the guest folder the chunks go to.
pub fn node_env(
    preload_dir: &str,
    profile_dir: &str,
    inherited: Option<&str>,
    mode: ProfileMode,
) -> NodeEnv {
    // `/` and not the host's separator: the guest is Linux, and Node on Windows takes `/` too.
    let mut node_options = require(&format!("{preload_dir}/preload.cjs"));
    if let Some(inherited) = inherited {
        node_options.push(' ');
        node_options.push_str(inherited);
    }
    NodeEnv {
        node_options,
        profile_dir: profile_dir.to_owned(),
        profile_mode: match mode {
            ProfileMode::Cpu => "cpu",
            ProfileMode::Coverage => "coverage",
        },
    }
}

impl NodeEnv {
    pub fn into_vars(self) -> [(&'static str, String); 3] {
        [
            ("NODE_OPTIONS", self.node_options),
            ("STOP_SLOP_PROFILE_DIR", self.profile_dir),
            ("STOP_SLOP_PROFILE_MODE", self.profile_mode.to_owned()),
        ]
    }
}

/// Node splits `NODE_OPTIONS` at spaces outside `"`, and inside `"` a `\` escapes the next
/// character: `C:\Users\a b\preload.cjs` is written `"C:\\Users\\a b\\preload.cjs"`.
fn require(preload: &str) -> String {
    let mut options = String::from("--require \"");
    for ch in preload.chars() {
        if matches!(ch, '\\' | '"') {
            options.push('\\');
        }
        options.push(ch);
    }
    options.push('"');
    options
}
