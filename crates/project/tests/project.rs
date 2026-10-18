use std::collections::BTreeSet;

use camino::{Utf8Path, Utf8PathBuf};
use kit::{EnumIndex, Fact, Facts, Package};
use project::{ProjectError, ProjectFacts, enum_index};
use repo::{Repo, Snapshot};
use testkit::TempRepo;

const ALL: [Fact; 3] = [Fact::Packages, Fact::Aliases, Fact::Enums];

fn open(t: &TempRepo) -> Repo {
    Repo::open(t.path()).unwrap()
}

fn paths(list: &[&str]) -> Vec<Utf8PathBuf> {
    list.iter().map(Utf8PathBuf::from).collect()
}

fn load(snapshot: &Snapshot, files: &[&str], needed: &[Fact]) -> ProjectFacts {
    ProjectFacts::load(snapshot, &paths(files), needed).unwrap()
}

fn deps(package: Option<&Package>) -> Vec<&str> {
    names(&package.expect("a package.json").dependencies)
}

fn names(set: &BTreeSet<String>) -> Vec<&str> {
    set.iter().map(String::as_str).collect()
}

/// (enum, member, file) of each member with the value `raw`.
fn members(index: &EnumIndex, raw: &str) -> Vec<(String, String, String)> {
    index
        .by_value(raw)
        .iter()
        .map(|member| {
            (
                member.enum_name.clone(),
                member.member.clone(),
                member.file.to_string(),
            )
        })
        .collect()
}

fn member(enum_name: &str, member: &str, file: &str) -> (String, String, String) {
    (enum_name.to_owned(), member.to_owned(), file.to_owned())
}

fn json_error(result: Result<ProjectFacts, ProjectError>) -> (String, String) {
    match result {
        Err(ProjectError::Json { path, message }) => (path.to_string(), message),
        other => panic!("expected a JSON error, got {other:?}"),
    }
}

#[test]
fn a_file_gets_the_packages_of_every_package_json_up_to_the_root() {
    let t = TempRepo::new();
    // A polyfill and Nest only in the root, as a monorepo may keep them.
    t.write(
        "package.json",
        r#"{"dependencies": {"@nestjs/core": "1"}, "devDependencies": {"events": "3"}}"#,
    )
    .write(
        "apps/x/package.json",
        r#"{
            "name": "x",
            "scripts": {"build": "tsc"},
            "jest": {"testEnvironment": "node"},
            "dependencies": {"a": "1"},
            "devDependencies": {"b": "1"},
            "peerDependencies": {"c": "1"},
            "optionalDependencies": {"d": "1"}
        }"#,
    )
    .write(
        "apps/x/tools/package.json",
        r#"{"dependencies": {"e": "1"}}"#,
    )
    .write("apps/x/src/deep/a.ts", "")
    .write("apps/x/tools/t.ts", "")
    .write("apps/y/src/b.ts", "")
    .write("c.ts", "")
    .commit("init");
    let files = [
        "apps/x/src/deep/a.ts",
        "apps/x/tools/t.ts",
        "apps/y/src/b.ts",
        "c.ts",
    ];
    let facts = load(&open(&t).worktree(), &files, &[Fact::Packages]);
    let package = |file: &str| facts.package(Utf8Path::new(file));

    let of_x = ["@nestjs/core", "a", "b", "c", "d", "events"];
    assert_eq!(deps(package("apps/x/src/deep/a.ts")), of_x);
    assert_eq!(deps(package("apps/x/package.json")), of_x);
    assert_eq!(
        deps(package("apps/x/tools/t.ts")),
        ["@nestjs/core", "a", "b", "c", "d", "e", "events"]
    );
    assert_eq!(deps(package("apps/y/src/b.ts")), ["@nestjs/core", "events"]);
    assert_eq!(deps(package("c.ts")), ["@nestjs/core", "events"]);
    assert!(package("other/z.ts").is_none());

    let bare = TempRepo::new();
    bare.write("a.ts", "").commit("init");
    let facts = load(&open(&bare).worktree(), &["a.ts"], &[Fact::Packages]);
    assert!(facts.package(Utf8Path::new("a.ts")).is_none());
}

#[test]
fn workspace_dependencies_go_into_workspace() {
    let t = TempRepo::new();
    t.write(
        "package.json",
        r#"{"devDependencies": {"@acme/tools": "workspace:*"}}"#,
    )
    .write(
        "apps/x/package.json",
        r#"{
            "dependencies": {"@acme/lib": "workspace:*", "left-pad": "1.0.0"},
            "devDependencies": {"@acme/test": "workspace:^1.0.0"}
        }"#,
    )
    .write("apps/x/a.ts", "")
    .commit("init");
    let facts = load(&open(&t).worktree(), &["apps/x/a.ts"], &[Fact::Packages]);
    let package = facts.package(Utf8Path::new("apps/x/a.ts")).unwrap();

    assert_eq!(
        names(&package.workspace),
        ["@acme/lib", "@acme/test", "@acme/tools"]
    );
    assert_eq!(
        names(&package.dependencies),
        ["@acme/lib", "@acme/test", "@acme/tools", "left-pad"]
    );
}

#[test]
fn aliases_come_from_the_nearest_tsconfig_and_its_local_extends() {
    let t = TempRepo::new();
    t.write(
        "tsconfig.base.json",
        "\u{feff}{\n\
         \x20 // the base of every app\n\
         \x20 \"compilerOptions\": { /* aliases */\n\
         \x20   \"paths\": {\"@/*\": [\"src/*\"], \"@lib/*\": [\"lib/*\"],},\n\
         \x20 },\n\
         }\n",
    )
    .write(
        "apps/x/tsconfig.json",
        r#"{"extends": "../../tsconfig.base.json"}"#,
    )
    .write(
        "apps/x/test/tsconfig.json",
        r#"{"extends": "../tsconfig.json"}"#,
    )
    .write(
        "apps/y/tsconfig.json",
        r#"{
            "extends": "../../tsconfig.base.json",
            "compilerOptions": {"paths": {"~/*": ["src/*"]}}
        }"#,
    )
    .write(
        "apps/z/tsconfig.json",
        r#"{"extends": ["./a", "./b.json"]}"#,
    )
    .write(
        "apps/z/a.json",
        r#"{"compilerOptions": {"paths": {"@a/*": ["a/*"]}}}"#,
    )
    .write(
        "apps/z/b.json",
        r#"{"compilerOptions": {"paths": {"@b/*": ["b/*"]}}}"#,
    )
    .write(
        "apps/p/tsconfig.json",
        r#"{"extends": "@tsconfig/node20/tsconfig.json"}"#,
    )
    .write("apps/m/tsconfig.json", r#"{"extends": "./missing.json"}"#)
    .write(
        "apps/o/tsconfig.json",
        r#"{"extends": "../../../../out.json"}"#,
    )
    .write("apps/c/tsconfig.json", r#"{"extends": "./b.json"}"#)
    .write("apps/c/b.json", r#"{"extends": "./tsconfig.json"}"#)
    .write(
        "apps/n/tsconfig.json",
        r#"{"extends": "../../tsconfig.base.json", "compilerOptions": {"paths": null}}"#,
    )
    .write(
        "apps/q/tsconfig.json",
        r#"{"extends": ["../z/a.json", "./null.json"]}"#,
    )
    .write(
        "apps/q/null.json",
        r#"{"compilerOptions": {"paths": null}}"#,
    )
    .write(
        "apps/k/tsconfig.json",
        r#"{"extends": "../../tsconfig.base.json", "compilerOptions": null}"#,
    );
    let files = [
        "apps/x/a.ts",
        "apps/x/test/a.ts",
        "apps/y/a.ts",
        "apps/z/a.ts",
        "apps/p/a.ts",
        "apps/m/a.ts",
        "apps/o/a.ts",
        "apps/c/a.ts",
        "apps/n/a.ts",
        "apps/q/a.ts",
        "apps/k/a.ts",
        "src/s.ts",
    ];
    let facts = load(&open(&t).worktree(), &files, &[Fact::Aliases]);
    let aliases = |file: &str| facts.aliases(Utf8Path::new(file));

    assert_eq!(aliases("apps/x/a.ts"), ["@/*", "@lib/*"]);
    assert_eq!(aliases("apps/x/test/a.ts"), ["@/*", "@lib/*"]);
    assert_eq!(aliases("apps/y/a.ts"), ["~/*"]);
    assert_eq!(aliases("apps/z/a.ts"), ["@b/*"]);
    assert_eq!(aliases("apps/k/a.ts"), ["@/*", "@lib/*"]);
    for file in [
        "apps/p/a.ts",
        "apps/m/a.ts",
        "apps/o/a.ts",
        "apps/c/a.ts",
        "apps/n/a.ts",
        "apps/q/a.ts",
        "src/s.ts",
    ] {
        assert!(aliases(file).is_empty(), "{file}");
    }
}

#[test]
fn load_builds_only_the_needed_facts() {
    let t = TempRepo::new();
    t.write("package.json", r#"{"dependencies": {"a": "1"}}"#)
        .write(
            "tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@/*": ["src/*"]}}}"#,
        )
        .write("src/e.ts", "export enum E { X = 'X' }")
        .commit("init");
    let worktree = open(&t).worktree();
    let file = Utf8Path::new("src/e.ts");

    let facts = load(&worktree, &["src/e.ts"], &[Fact::Packages]);
    assert_eq!(deps(facts.package(file)), ["a"]);
    assert!(facts.aliases(file).is_empty());
    assert!(facts.enums().by_value("X").is_empty());

    let facts = load(&worktree, &["src/e.ts"], &[Fact::Aliases]);
    assert!(facts.package(file).is_none());
    assert_eq!(facts.aliases(file), ["@/*"]);

    let facts = load(&worktree, &["src/e.ts"], &[Fact::Enums]);
    assert_eq!(members(facts.enums(), "X"), [member("E", "X", "src/e.ts")]);
    assert!(facts.package(file).is_none());
    assert!(facts.aliases(file).is_empty());

    let facts = load(&worktree, &["src/e.ts"], &[]);
    assert!(facts.package(file).is_none());
    assert!(facts.aliases(file).is_empty());
    assert!(facts.enums().by_value("X").is_empty());

    t.write("package.json", "{");
    let needed = [Fact::Aliases, Fact::Enums];
    assert!(ProjectFacts::load(&worktree, &paths(&["src/e.ts"]), &needed).is_ok());
}

#[test]
fn the_enum_index_holds_string_members_of_files_with_the_word_enum() {
    let t = TempRepo::new();
    t.write(
        "src/source.enum.ts",
        "export enum Source { Dashboard = 'DASHBOARD', Api = \"API\", Count = 1, Tpl = `TPL` }",
    )
    .write(
        "src/local.ts",
        "const enum Mode { Fast = 'fast' }\nexport declare enum Flag { On = 'on' }",
    )
    .write("src/name.ts", "enum Weird { 'two words' = 'TW' }")
    .write("src/other.ts", "export enum Other { Api = 'API' }")
    .write("src/word.ts", "z.enum(['WORD']);\nconst enumerate = 1;")
    .write(
        "src/ns.ts",
        "namespace N { export enum Inner { X = 'NS' } }",
    )
    .write("types/gen.d.ts", "export declare enum Gen { G = 'GEN' }")
    .write("docs/a.md", "An enum of sources.")
    .write(
        "src/broken.ts",
        "export enum Broken { B = 'BROKEN' }\nconst = ;",
    )
    .commit("init")
    .write("src/new.enum.ts", "export enum New { N = 'NEW' }");
    let facts = load(&open(&t).worktree(), &[], &[Fact::Enums]);
    let members = |raw: &str| members(facts.enums(), raw);

    assert_eq!(
        members("DASHBOARD"),
        [member("Source", "Dashboard", "src/source.enum.ts")]
    );
    assert_eq!(
        members("TPL"),
        [member("Source", "Tpl", "src/source.enum.ts")]
    );
    assert_eq!(
        members("API"),
        [
            member("Other", "Api", "src/other.ts"),
            member("Source", "Api", "src/source.enum.ts"),
        ]
    );
    assert_eq!(members("fast"), [member("Mode", "Fast", "src/local.ts")]);
    assert_eq!(members("on"), [member("Flag", "On", "src/local.ts")]);
    assert_eq!(members("TW"), [member("Weird", "two words", "src/name.ts")]);
    assert_eq!(members("NEW"), [member("New", "N", "src/new.enum.ts")]);
    for raw in ["1", "WORD", "NS", "GEN"] {
        assert!(members(raw).is_empty(), "{raw}");
    }
}

#[test]
fn rev_takes_the_facts_of_the_revision() {
    let t = TempRepo::new();
    t.write("package.json", r#"{"dependencies": {"a": "1"}}"#)
        .write(
            "tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@old/*": ["src/*"]}}}"#,
        )
        .write("src/e.ts", "export enum E { X = 'OLD' }")
        .commit("init")
        .write("package.json", r#"{"dependencies": {"b": "1"}}"#)
        .write(
            "tsconfig.json",
            r#"{"compilerOptions": {"paths": {"@new/*": ["src/*"]}}}"#,
        )
        .write("src/e.ts", "export enum E { X = 'NEW' }")
        .write("src/u.ts", "export enum U { Y = 'UNTRACKED' }");
    let repo = open(&t);
    let file = Utf8Path::new("src/e.ts");

    let facts = load(&repo.at_rev("HEAD").unwrap(), &["src/e.ts"], &ALL);
    assert_eq!(deps(facts.package(file)), ["a"]);
    assert_eq!(facts.aliases(file), ["@old/*"]);
    assert_eq!(
        members(facts.enums(), "OLD"),
        [member("E", "X", "src/e.ts")]
    );
    assert!(facts.enums().by_value("NEW").is_empty());
    assert!(facts.enums().by_value("UNTRACKED").is_empty());

    let facts = load(&repo.worktree(), &["src/e.ts"], &ALL);
    assert_eq!(deps(facts.package(file)), ["b"]);
    assert_eq!(facts.aliases(file), ["@new/*"]);
    assert!(facts.enums().by_value("OLD").is_empty());
    assert_eq!(
        members(facts.enums(), "NEW"),
        [member("E", "X", "src/e.ts")]
    );
    assert_eq!(
        members(facts.enums(), "UNTRACKED"),
        [member("U", "Y", "src/u.ts")]
    );
}

#[test]
fn a_broken_json_file_is_an_error_with_its_path() {
    let t = TempRepo::new();
    t.write("apps/x/package.json", r#"{"dependencies": {"#)
        .write("apps/x/a.ts", "");
    let worktree = open(&t).worktree();
    let files = paths(&["apps/x/a.ts"]);
    let error = |needed: &[Fact]| json_error(ProjectFacts::load(&worktree, &files, needed));

    let (path, message) = error(&[Fact::Packages]);
    assert_eq!(path, "apps/x/package.json");
    assert!(message.contains("line"), "{message}");

    t.write("apps/x/package.json", r#"{"dependencies": {"a": 1}}"#);
    let (path, _) = error(&[Fact::Packages]);
    assert_eq!(path, "apps/x/package.json");

    t.write("tsconfig.base.json", r#"{"compilerOptions": {"#)
        .write(
            "apps/x/tsconfig.json",
            r#"{"extends": "../../tsconfig.base.json"}"#,
        );
    let (path, _) = error(&[Fact::Aliases]);
    assert_eq!(path, "tsconfig.base.json");
}

#[test]
fn enum_index_reads_sources_by_their_extension() {
    let sources = [
        (Utf8Path::new("a.ts"), "export enum A { X = 'x' }"),
        (
            Utf8Path::new("b.tsx"),
            "enum B { Y = 'x' } const c = <div/>;",
        ),
        (Utf8Path::new("c.json"), "{}"),
    ];
    let index = enum_index(&sources);

    assert_eq!(
        members(&index, "x"),
        [member("A", "X", "a.ts"), member("B", "Y", "b.tsx")]
    );
}
