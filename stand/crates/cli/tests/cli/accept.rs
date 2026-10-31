//! `accept` of a measure of the last passed run.

use std::fs;

use cli::Exit;
use serde_json::Value;

use super::{World, fake};

/// `accepted.json` of the one session of the world (`store`, a law of the blueprint).
fn acceptances(world: &World) -> Vec<Value> {
    let session = world.sessions().remove(0);
    let Ok(text) = fs::read_to_string(session.join("accepted.json")) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap()
}

#[test]
fn accept_appends_a_bound_with_its_reason_and_refuses_the_rest() {
    let world = World::new();
    let fake = fake(Vec::new());
    world.run(&fake, &["snapshot"]);
    let args = ["accept", "stand.wall", "--allow", "+15%", "--reason", "x"];
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Refused, "no run passed yet: {}", ran.err);
    assert_eq!(ran.next(), ["stop-slop-stand guide expect"]);
    world.repo.write("main.js", "console.log('v2')\n");
    let first = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let tree = world.report(&first)["head"]["tree"]
        .as_str()
        .unwrap()
        .to_owned();
    // A bound is kept and printed in the grammar of `--allow`, as `next` writes it.
    let args = [
        "accept",
        "stand.wall",
        "--allow",
        "+015%",
        "--reason",
        "one more lookup",
    ];
    let ran = world.run(&fake, &args);
    assert_eq!(ran.exit, Exit::Passed, "{}", ran.err);
    assert_eq!(
        ran.out,
        format!("accepted stand.wall +15% for head {tree}\n")
    );
    let args = [
        "accept",
        "stand.wall",
        "--allow",
        "-10%",
        "--reason",
        "a cache",
    ];
    assert_eq!(world.run(&fake, &args).exit, Exit::Passed);
    let accepted = acceptances(&world);
    let kept: Vec<(&str, &str, &str, &str)> = accepted
        .iter()
        .map(|entry| {
            let field = |key: &str| entry[key].as_str().unwrap();
            (
                field("measure"),
                field("allow"),
                field("reason"),
                field("headTree"),
            )
        })
        .collect();
    assert_eq!(
        kept,
        [
            ("stand.wall", "+15%", "one more lookup", tree.as_str()),
            ("stand.wall", "-10%", "a cache", tree.as_str()),
        ]
    );
    assert!(accepted.iter().all(|entry| entry["acceptedAt"].is_string()));
    // The next run of this head grades by the last acceptance, and counts both.
    let next = world
        .run(&fake, &["run", "--before", "session", "--json"])
        .line();
    let report = world.report(&next);
    let wall = report["measures"]
        .as_array()
        .unwrap()
        .iter()
        .find(|measure| measure["name"] == "stand.wall")
        .unwrap();
    let expect = &wall["expect"];
    assert_eq!(
        (
            &expect["change"],
            &expect["postHoc"],
            &expect["acceptances"]
        ),
        (&Value::from("-10%"), &Value::from(true), &Value::from(2)),
        "{expect}"
    );
    // One flag without the other, a bound wider than `max_tolerance`, `unknown`, a bound out of
    // the grammar and a measure the last report does not have: 2, and nothing appended.
    let refusals: [(&[&str], &[&str]); 6] = [
        (
            &["accept", "stand.wall", "--allow", "+15%"],
            &["stop-slop-stand guide"],
        ),
        (
            &["accept", "stand.wall", "--reason", "why"],
            &["stop-slop-stand guide"],
        ),
        (
            &[
                "accept",
                "stand.wall",
                "--allow",
                "+40%",
                "--reason",
                "wide",
            ],
            &[],
        ),
        (
            &[
                "accept",
                "stand.wall",
                "--allow",
                "unknown",
                "--reason",
                "x",
            ],
            &["stop-slop-stand guide expect"],
        ),
        (
            &["accept", "stand.wall", "--allow", "15%", "--reason", "x"],
            &["stop-slop-stand guide expect"],
        ),
        (
            &["accept", "no.such", "--allow", "+15%", "--reason", "x"],
            &["stop-slop-stand guide expect"],
        ),
    ];
    for (args, next) in refusals {
        let ran = world.run(&fake, args);
        assert_eq!(ran.exit, Exit::Refused, "{args:?}");
        assert_eq!(ran.next(), next, "{args:?}: {}", ran.err);
    }
    let wide = world.run(&fake, refusals[2].0);
    assert!(wide.err.contains("max_tolerance, 25%"), "{}", wide.err);
    assert_eq!(acceptances(&world).len(), 2);
}
