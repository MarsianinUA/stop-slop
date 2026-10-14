use camino::Utf8Path;
use kit::oxc::semantic::SemanticBuilder;
use kit::oxc::span::SourceType;
use kit::prelude::*;
use kit::{EnumIndex, EnumMember, LineIndex, RoleDef, Roles, StaticFacts, build, suffix_of};

#[test]
fn role_of_takes_the_first_role_matching_a_suffix_or_a_folder() {
    let roles = Roles {
        tests: RoleDef {
            suffixes: &["spec", "test", "e2e-spec"],
            dirs: &["__tests__"],
        },
        types: RoleDef {
            suffixes: &["type", "types", "interface", "enum"],
            dirs: &[],
        },
        constants: RoleDef {
            suffixes: &["constant", "constants"],
            dirs: &[],
        },
        functions: RoleDef {
            suffixes: &["util", "mapper", "config", "schema"],
            dirs: &[],
        },
    };

    for (path, role) in [
        ("src/a.spec.ts", Some(Role::Tests)),
        ("src/__tests__/a.ts", Some(Role::Tests)),
        ("src/__tests__/a.types.ts", Some(Role::Tests)),
        ("src/a.types.ts", Some(Role::Types)),
        ("src/a.constants.ts", Some(Role::Constants)),
        ("src/a.util.ts", Some(Role::Functions)),
        ("src/a.service.ts", None),
    ] {
        assert_eq!(roles.role_of(Utf8Path::new(path)), role, "{path}");
    }
}

#[test]
fn suffix_of_is_the_extension_of_the_stem() {
    for (path, suffix) in [
        ("order.response.ts", Some("response")),
        ("a.ts", None),
        (".types.ts", None),
    ] {
        assert_eq!(suffix_of(Utf8Path::new(path)), suffix, "{path}");
    }
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Limit {
    #[expect(dead_code, reason = "the tests only build the rule")]
    max: u32,
}

impl Rule for Limit {}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct Check {}

impl Rule for Check {}

#[test]
fn build_reads_options_and_names_the_bad_key() {
    assert!(build::<Limit>("max = 1").is_ok());

    let Err(wrong_type) = build::<Limit>("max = \"x\"") else {
        panic!("a string for a u32 option must fail");
    };
    assert!(wrong_type.to_string().contains("max"), "{wrong_type}");

    let Err(unknown) = build::<Limit>("max = 1\ncolour = \"red\"") else {
        panic!("an unknown option must fail");
    };
    assert!(unknown.to_string().contains("colour"), "{unknown}");

    assert!(build::<Check>("").is_ok());
}

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
struct ReportTwice {}

impl Rule for ReportTwice {
    const USES: &'static [Fact] = &[Fact::Aliases];

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        cx.report(Span::new(0, 3));
        cx.report(Span::new(4, 5)).arg("name", "x").fix([Edit {
            span: Span::new(4, 5),
            text: "y".to_string(),
        }]);
    }
}

#[test]
fn a_bare_report_is_a_finding_and_arg_and_fix_add_to_it() {
    let text = "let x = 1;";
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, SourceType::ts()).parse();
    let semantic = SemanticBuilder::new().build(&parsed.program).semantic;
    let lines = LineIndex::new(text);
    let file = File {
        path: Utf8Path::new("src/a.ts"),
        text,
        lines: &lines,
        semantic: &semantic,
        role: None,
    };
    let facts = StaticFacts {
        package: None,
        aliases: Vec::new(),
        enums: EnumIndex::new(Vec::new()),
    };
    let rule = build::<ReportTwice>("").unwrap();
    let mut out = Vec::new();

    rule.on_file(&mut Cx::new(&file, &facts, &mut out));

    assert_eq!(rule.uses(), [Fact::Aliases]);
    let [bare, full] = out.as_slice() else {
        panic!("expected two reports, got {out:?}");
    };
    assert_eq!(bare.span, Span::new(0, 3));
    assert!(bare.args.is_empty() && bare.edits.is_empty());
    assert_eq!(full.args, [("name", "x".to_string())]);
    assert_eq!(full.edits[0].text, "y");
}

#[test]
fn enum_index_groups_members_by_value() {
    let member = |enum_name: &str| EnumMember {
        enum_name: enum_name.to_string(),
        member: "A".to_string(),
        file: "src/a.enum.ts".into(),
    };
    let index = EnumIndex::new(vec![
        ("a".to_string(), member("Mode")),
        ("a".to_string(), member("Kind")),
        ("b".to_string(), member("Mode")),
    ]);

    let names: Vec<_> = index
        .by_value("a")
        .iter()
        .map(|m| m.enum_name.as_str())
        .collect();
    assert_eq!(names, ["Mode", "Kind"]);
    assert!(index.by_value("c").is_empty());
}

#[test]
fn line_index_counts_columns_in_chars_and_keeps_the_line_ending() {
    let text = "привет 😀 x";
    let index = LineIndex::new(text);
    let x = text.find('x').unwrap() as u32;
    let position = index.position(x);
    assert_eq!((position.line, position.column), (1, 10));

    let two_lines = "я\nб x";
    let x = LineIndex::new(two_lines).position(two_lines.find('x').unwrap() as u32);
    assert_eq!((x.line, x.column), (2, 3));

    let crlf = LineIndex::new("a\r\nb\r\n");
    assert_eq!(crlf.line_ending(), "\r\n");
    let b = crlf.position(3);
    assert_eq!((b.line, b.column), (2, 1));

    let lf = LineIndex::new("a\n");
    assert_eq!(lf.line_ending(), "\n");
    let end = lf.position(2);
    assert_eq!((end.line, end.column), (2, 1));

    let empty = LineIndex::new("");
    assert_eq!(empty.line_ending(), "\n");
    let start = empty.position(0);
    assert_eq!((start.line, start.column), (1, 1));
}
