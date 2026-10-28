//! The keys the stand reads, fail-closed: a key it does not know is refused with its path, so a
//! Compose feature the stand would silently skip never reaches a run.

use crate::Invalid;
use crate::tree::Field;

/// What a position of the file takes. `allow` looks at keys, and at the type only where keys are
/// fixed: the rest is `bind`'s.
enum Shape {
    /// A mapping of these keys; with `extensions` also of `x-*` keys, which nothing reads.
    Keys {
        keys: &'static [(&'static str, Shape)],
        extensions: bool,
    },
    /// A mapping of names, such as services, each value of this shape.
    Names(&'static Shape),
    /// A list, each item of this shape.
    Items(&'static Shape),
    Value,
}

use Shape::{Items, Keys, Names, Value};

const ROOT: Shape = Keys {
    keys: &[
        ("services", Names(&SERVICE)),
        ("volumes", Names(&VOLUME)),
        ("name", Value),
        ("x-stand", X_STAND),
    ],
    extensions: true,
};

const SERVICE: Shape = Keys {
    keys: &[
        ("image", Value),
        ("command", Value),
        ("entrypoint", Value),
        ("environment", Value),
        ("working_dir", Value),
        ("volumes", Value),
        ("expose", Value),
        ("depends_on", Names(&DEPENDENCY)),
        ("healthcheck", HEALTHCHECK),
        ("stop_signal", Value),
        ("stop_grace_period", Value),
        ("deploy", DEPLOY),
        (
            "x-stand",
            Keys {
                keys: &[("profile", Value), ("network", Value)],
                extensions: false,
            },
        ),
    ],
    extensions: true,
};

/// `pgdata: {}`: the stand gives every volume its own disk, so a driver or options mean nothing.
const VOLUME: Shape = Keys {
    keys: &[],
    extensions: false,
};

const DEPENDENCY: Shape = Keys {
    keys: &[("condition", Value)],
    extensions: false,
};

const HEALTHCHECK: Shape = Keys {
    keys: &[
        ("test", Value),
        ("interval", Value),
        ("timeout", Value),
        ("retries", Value),
        ("start_period", Value),
    ],
    extensions: false,
};

const DEPLOY: Shape = Keys {
    keys: &[
        (
            "resources",
            Keys {
                keys: &[(
                    "limits",
                    Keys {
                        keys: &[("cpus", Value), ("memory", Value)],
                        extensions: false,
                    },
                )],
                extensions: false,
            },
        ),
        ("replicas", Value),
    ],
    extensions: false,
};

const X_STAND: Shape = Keys {
    keys: &[
        ("workload", Value),
        ("repeat", Value),
        ("setup_timeout", Value),
        ("timeout", Value),
        ("probes", Names(&PROBE)),
        (
            "expect",
            Keys {
                keys: &[("cost", Items(&COST)), ("unchanged", Items(&UNCHANGED))],
                extensions: false,
            },
        ),
        (
            "fault",
            Keys {
                keys: &[("target", Value), ("mode", Value), ("delay", Value)],
                extensions: false,
            },
        ),
        ("second_pass", Value),
    ],
    extensions: false,
};

const PROBE: Shape = Keys {
    keys: &[
        ("in", Value),
        ("at", Value),
        ("run", Value),
        ("key", Value),
        ("ignore", Value),
    ],
    extensions: false,
};

const COST: Shape = Keys {
    keys: &[("measure", Value), ("change", Value), ("reason", Value)],
    extensions: false,
};

const UNCHANGED: Shape = Keys {
    keys: &[("file", Value), ("reason", Value)],
    extensions: false,
};

/// Keys a Compose file often has, with why the stand does without them.
const REFUSED: [(&str, &str); 3] = [
    (
        "build",
        "the stand builds no image: build it elsewhere and name it in `image`",
    ),
    (
        "ports",
        "the stand publishes no port: other services reach this one by its name and `expose`",
    ),
    (
        "<<",
        "YAML merge keys are not supported: write the keys out",
    ),
];

/// A document that is not a mapping has no services, which `bind` reports.
pub(crate) fn check(root: &Field) -> Result<(), Invalid> {
    if !root.is_map() {
        return Ok(());
    }
    walk(root, &ROOT)
}

fn walk(field: &Field, shape: &Shape) -> Result<(), Invalid> {
    match shape {
        Value => Ok(()),
        Names(inner) if field.is_map() => {
            for (_, value) in field.entries()? {
                walk(&value, inner)?;
            }
            Ok(())
        }
        Keys { keys, extensions } => {
            for (key, value) in field.entries()? {
                match keys.iter().find(|(name, _)| *name == key) {
                    Some((_, inner)) => walk(&value, inner)?,
                    None if *extensions && key.starts_with("x-") => {}
                    None => return Err(value.invalid(unknown(key, keys, *extensions))),
                }
            }
            Ok(())
        }
        Items(inner) if field.is_list() => {
            for item in field.items()? {
                walk(&item, inner)?;
            }
            Ok(())
        }
        Names(_) | Items(_) => Ok(()),
    }
}

fn unknown(key: &str, keys: &[(&str, Shape)], extensions: bool) -> String {
    if let Some((_, why)) = REFUSED.iter().find(|(refused, _)| *refused == key) {
        return format!("`{key}`: {why}");
    }
    let mut known: Vec<String> = keys.iter().map(|(name, _)| format!("`{name}`")).collect();
    if extensions {
        known.push("`x-*`".to_owned());
    }
    let takes = if known.is_empty() {
        "no key".to_owned()
    } else {
        known.join(", ")
    };
    let outside = if key.starts_with("x-") && !extensions {
        "; `x-*` keys go at the top level or in a service"
    } else {
        ""
    };
    format!("unknown key `{key}`: here the stand takes {takes}{outside}")
}
