use std::collections::BTreeSet;

use camino::Utf8Path;
use kit::oxc::ast::ast::{Expression, TSType};
use kit::oxc::parser::ParseOptions;
use kit::oxc::semantic::SemanticBuilder;
use kit::oxc::span::SourceType;
use kit::prelude::*;
use kit::{
    EnumIndex, EnumMember, LineIndex, Package, RoleDef, Roles, StaticFacts, build, suffix_of,
};

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
    let facts = no_facts();
    let rule = build::<ReportTwice>("").unwrap();
    let mut out = Vec::new();

    with_file("let x = 1;", |file| {
        rule.on_file(&mut Cx::new(file, &facts, &mut out));
    });

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

/// Parses `text` the way the engine does.
fn with_file(text: &str, check: impl FnOnce(&File<'_>)) {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, SourceType::ts())
        .with_options(ParseOptions {
            preserve_parens: false,
            ..ParseOptions::default()
        })
        .parse();
    assert!(
        parsed.diagnostics.is_empty(),
        "{text}: {:?}",
        parsed.diagnostics
    );
    let semantic = SemanticBuilder::new()
        .with_build_nodes(true)
        .build(&parsed.program)
        .semantic;
    let lines = LineIndex::new(text);
    let file = File {
        path: Utf8Path::new("src/a.ts"),
        text,
        lines: &lines,
        semantic: &semantic,
        role: None,
    };
    check(&file);
}

fn alias_type<'a>(file: &File<'a>, name: &str) -> &'a TSType<'a> {
    file.semantic
        .nodes()
        .iter()
        .find_map(|node| match node.kind() {
            AstKind::TSTypeAliasDeclaration(alias) if alias.id.name == name => {
                Some(&alias.type_annotation)
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no type {name}"))
}

fn first_assertion<'a>(file: &File<'a>) -> (&'a Expression<'a>, &'a TSType<'a>) {
    file.semantic
        .nodes()
        .iter()
        .find_map(|node| type_assertion(node.kind()))
        .unwrap()
}

fn source<'a>(file: &File<'a>, node: &impl GetSpan) -> &'a str {
    node.span().source_text(file.text)
}

const SAFETY_CASES: &[(&str, bool)] = &[
    ("// SAFETY: x\nvalue!;", true),
    ("value!; // SAFETY: x", true),
    ("/* SAFETY: x */\nvalue!;", true),
    ("/**\n * SAFETY: x\n */\nvalue!;", true),
    ("// SAFETY: a\n// b\nvalue!;", true),
    ("// SAFETY: a\n// eslint-disable-next-line\nvalue!;", true),
    ("/* SAFETY: x */ value!;", true),
    ("function f() {\n  // SAFETY: x\n  return value!;\n}", true),
    ("const user = {\n  a: 1,\n} as User; // SAFETY: x", true),
    ("// SAFETY:\nvalue!;", false),
    ("// SAFETY:   \nvalue!;", false),
    ("// safety: x\nvalue!;", false),
    ("// SAFETY: x\n\nvalue!;", false),
    ("foo(); // SAFETY: x\nvalue!;", false),
    ("// SAFETY: x\nconst total = first +\n  value!;", false),
    ("const user = { // SAFETY: x\n  a: 1,\n} as User;", false),
    ("value!;", false),
];

fn assert_safety_cases(line_ending: &str) {
    for &(case, expected) in SAFETY_CASES {
        let text = case.replace('\n', line_ending);
        with_file(&text, |file| {
            let span = file
                .semantic
                .nodes()
                .iter()
                .map(|node| node.kind())
                .find(|kind| {
                    matches!(
                        kind,
                        AstKind::TSNonNullExpression(_) | AstKind::TSAsExpression(_)
                    )
                })
                .unwrap()
                .span();
            assert_eq!(has_safety_comment(file, span), expected, "{case:?}");
        });
    }
}

#[test]
fn safety_comment_counts_right_above_the_line_or_at_its_end() {
    assert_safety_cases("\n");
}

#[test]
fn safety_comment_gives_the_same_answers_with_crlf() {
    assert_safety_cases("\r\n");
}

#[test]
fn type_assertion_finds_as_and_angle_brackets_and_follows_a_chain() {
    for text in ["x as T;", "<T>x;"] {
        with_file(text, |file| {
            let (value, ty) = first_assertion(file);
            assert_eq!(
                (source(file, value), source(file, ty)),
                ("x", "T"),
                "{text}"
            );
        });
    }
    for text in ["x satisfies T;", "x!;"] {
        with_file(text, |file| {
            let found = file
                .semantic
                .nodes()
                .iter()
                .any(|node| type_assertion(node.kind()).is_some());
            assert!(!found, "{text}");
        });
    }

    for (text, is_const) in [
        ("x as const;", true),
        ("<const>x;", true),
        ("x as T;", false),
    ] {
        with_file(text, |file| {
            assert_eq!(
                is_const_assertion(first_assertion(file).1),
                is_const,
                "{text}"
            );
        });
    }

    with_file("(x as unknown) as T;", |file| {
        let (outer, ty) = first_assertion(file);
        assert_eq!(source(file, ty), "T");
        let (inner, inner_ty) = expression_assertion(outer).unwrap();
        assert_eq!(
            (source(file, inner), source(file, inner_ty)),
            ("x", "unknown")
        );
        assert!(expression_assertion(inner).is_none());
    });
}

#[test]
fn lib_type_is_a_reference_the_file_does_not_declare_or_import() {
    with_file("type A = Record<string, number>;", |file| {
        let ty = alias_type(file, "A");
        let record = lib_type(file, ty, "Record").unwrap();
        assert_eq!(source(file, record), "Record<string, number>");
        assert!(lib_type(file, ty, "Readonly").is_none());
    });
    for text in [
        "import { Record } from './record';\ntype A = Record<string, number>;",
        "type Record<K, V> = Map<K, V>;\ntype A = Record<string, number>;",
        "type A<Record> = Record;",
    ] {
        with_file(text, |file| {
            assert!(
                lib_type(file, alias_type(file, "A"), "Record").is_none(),
                "{text}"
            );
        });
    }
}

#[test]
fn unwrapped_type_drops_parentheses_readonly_and_lib_wrappers() {
    for (text, unwrapped) in [
        (
            "type A = Readonly<Partial<(Record<string, unknown>)>>;",
            "Record<string, unknown>",
        ),
        ("type A = readonly string[];", "string[]"),
        ("type A<T> = NonNullable<Required<T>>;", "T"),
        (
            "type Readonly<T> = T;\ntype A = Readonly<{ a: 1 }>;",
            "Readonly<{ a: 1 }>",
        ),
    ] {
        with_file(text, |file| {
            let ty = unwrapped_type(file, alias_type(file, "A"));
            assert_eq!(source(file, ty), unwrapped, "{text}");
        });
    }
}

#[test]
fn open_key_admits_any_string_number_or_symbol() {
    for (text, open) in [
        ("type K = string;", true),
        ("type K = number;", true),
        ("type K = symbol;", true),
        ("type K = PropertyKey;", true),
        ("type K = 'a' | string;", true),
        ("type K = (string);", true),
        ("type K = any;", true),
        ("type K = keyof any;", true),
        ("type K = `${string}`;", true),
        ("type K = `id-${number}`;", true),
        ("type K = string & {};", true),
        ("type K = 'a' | (string & {});", true),
        ("type K = 'a' | 'b';", false),
        ("type K = keyof { a: 1 };", false),
        ("type K = `id-${'a' | 'b'}`;", false),
        ("type K = `id`;", false),
        ("type K = string & { brand: 'Id' };", false),
        ("type Keys = string;\ntype K = Keys;", false),
        ("type PropertyKey = 'a';\ntype K = PropertyKey;", false),
    ] {
        with_file(text, |file| {
            assert_eq!(is_open_key(file, alias_type(file, "K")), open, "{text}");
        });
    }
}

#[test]
fn known_value_has_its_type_in_its_own_syntax() {
    for (text, known) in [
        ("let probe = { a: 1 };", true),
        ("let probe = [1];", true),
        ("let probe = () => 1;", true),
        ("let probe = function () {};", true),
        ("let probe = class {};", true),
        ("let probe = new Map();", true),
        ("let probe = 's';", true),
        ("let probe = `t${x}`;", true),
        ("let probe = 1;", true),
        ("let probe = 1n;", true),
        ("let probe = true;", true),
        ("let probe = /a/;", true),
        ("let probe = -1;", true),
        ("let probe = !x;", true),
        ("let probe = void 0;", false),
        ("let probe = null;", false),
        ("let probe = undefined;", false),
        ("let probe = f();", false),
        ("let probe = a.b;", false),
        ("function f(param) {\n  let probe = param;\n}", false),
        (
            "import { imported } from './a';\nlet probe = imported;",
            false,
        ),
        ("let probe = ({ a: 1 });", true),
        ("const known = { a: 1 };\nlet probe = known as T;", true),
        ("let probe = { a: 1 } satisfies T;", true),
        ("const known = { a: 1 };\nlet probe = known!;", true),
        ("const plain = { a: 1 };\nlet probe = plain;", true),
        ("const typed: T = { a: 1 };\nlet probe = typed;", false),
        ("let changing = { a: 1 };\nlet probe = changing;", false),
        ("const { a } = obj;\nlet probe = a;", false),
        ("const a = b;\nconst b = a;\nlet probe = a;", false),
    ] {
        with_file(text, |file| {
            let init = file
                .semantic
                .nodes()
                .iter()
                .find_map(|node| match node.kind() {
                    AstKind::VariableDeclarator(declarator)
                        if declarator.id.get_identifier_name() == Some("probe".into()) =>
                    {
                        declarator.init.as_ref()
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(is_known_value(file, init), known, "{text}");
        });
    }
}

#[test]
fn is_exported_sees_export_statements_specifiers_and_namespaces() {
    let text = "export const a = 1, a2 = 2;
export function f() {}
export class C {}
export default class D {}
export type T = 1;
export interface I {}
export enum E { X }
const b = 1; export { b };
const c = 1; export { c as cee };
type U = 1; export type { U };
const { d, e } = obj; export { d };
export const { g, h } = obj;
namespace N { export const inner = 1; const hidden = 2; }
import { imp } from './x'; export { imp };
export { re } from './y';
const local = 1;
function used() { const a = 1; return a + local; }";
    with_file(text, |file| {
        let scoping = file.semantic.scoping();
        let used = text.find("function used").unwrap() as u32;
        let mut exported: Vec<_> = scoping
            .symbol_ids()
            .map(|symbol| {
                let declaration = scoping.symbol_declaration(symbol);
                let name = match scoping.symbol_name(symbol) {
                    "a" if file.semantic.nodes().get_node(declaration).span().start > used => {
                        "a in used"
                    }
                    name => name,
                };
                (name, is_exported(file, symbol))
            })
            .collect();
        exported.sort_unstable();

        let mut expected: Vec<_> = [
            "a", "a2", "f", "C", "D", "T", "I", "E", "b", "c", "U", "d", "g", "h", "inner", "imp",
        ]
        .map(|name| (name, true))
        .into_iter()
        .chain(["X", "e", "N", "hidden", "local", "used", "a in used"].map(|name| (name, false)))
        .collect();
        expected.sort_unstable();
        assert_eq!(exported, expected);
    });

    for (text, name) in [
        ("const dflt = 1; export default dflt;", "dflt"),
        ("const assigned = 1; export = assigned;", "assigned"),
    ] {
        with_file(text, |file| {
            let scoping = file.semantic.scoping();
            let symbol = scoping.get_root_binding(name.into()).unwrap();
            assert!(is_exported(file, symbol), "{text}");
        });
    }
}

#[test]
fn is_exported_node_takes_the_own_name_the_default_export_or_an_exported_variable() {
    for (text, exported) in [
        ("export function f() {}", true),
        ("export default function () {}", true),
        ("export const g = () => 1;", true),
        ("export const h = function () {};", true),
        ("export default class {}", true),
        ("export const K = class B {};", true),
        ("class M {} export { M };", true),
        ("export interface I {}", true),
        ("export default interface Q {}", true),
        ("export type T = 1;", true),
        ("export enum E { X }", true),
        ("export const typed: F = (x) => x;", true),
        ("export const w = wrap(() => 1);", false),
        ("class L {}", false),
        ("interface J {}", false),
        ("const local = () => 1;", false),
        ("export const api = { get() {} };", false),
    ] {
        with_file(text, |file| {
            let node = file
                .semantic
                .nodes()
                .iter()
                .find(|node| {
                    matches!(
                        node.kind(),
                        AstKind::Function(_)
                            | AstKind::ArrowFunctionExpression(_)
                            | AstKind::Class(_)
                            | AstKind::TSInterfaceDeclaration(_)
                            | AstKind::TSTypeAliasDeclaration(_)
                            | AstKind::TSEnumDeclaration(_)
                    )
                })
                .unwrap();
            assert_eq!(is_exported_node(file, node), exported, "{text}");
        });
    }
}

/// `events` is a declared polyfill; `@acme/db` is a workspace package and a key of `paths`.
fn acme_facts() -> StaticFacts {
    StaticFacts {
        package: Some(Package {
            dependencies: BTreeSet::from(["events".to_string()]),
            workspace: BTreeSet::from(["@acme/db".to_string()]),
        }),
        aliases: ["@/*", "@acme/shared/*", "@acme/db", "~config", "*"]
            .map(String::from)
            .into(),
        enums: EnumIndex::new(Vec::new()),
    }
}

fn no_facts() -> StaticFacts {
    StaticFacts {
        package: None,
        aliases: Vec::new(),
        enums: EnumIndex::new(Vec::new()),
    }
}

#[test]
fn import_kind_takes_relative_node_workspace_alias_then_external() {
    let acme = acme_facts();
    let none = no_facts();
    with_file("", |file| {
        for (source, kind) in [
            ("node:fs", ImportKind::Node),
            ("node:test", ImportKind::Node),
            ("fs", ImportKind::Node),
            ("fs/promises", ImportKind::Node),
            ("util/types", ImportKind::Node),
            ("events", ImportKind::External),
            ("fs/x", ImportKind::External),
            ("lodash", ImportKind::External),
            ("@nestjs/core", ImportKind::External),
            ("~config/x", ImportKind::External),
            ("@acme/db", ImportKind::Workspace),
            ("@acme/db/schema", ImportKind::Workspace),
            ("@/orders/x", ImportKind::Alias),
            ("@acme/shared/types", ImportKind::Alias),
            ("~config", ImportKind::Alias),
            ("./order", ImportKind::Relative),
            ("../lib/x", ImportKind::Relative),
            (".", ImportKind::Relative),
            ("..", ImportKind::Relative),
            ("/abs/x", ImportKind::Relative),
        ] {
            assert_eq!(import_kind(file, &acme, source), kind, "{source}");
        }
        for (source, kind) in [
            ("events", ImportKind::Node),
            ("@acme/db", ImportKind::External),
            ("@/x", ImportKind::External),
        ] {
            assert_eq!(
                import_kind(file, &none, source),
                kind,
                "{source} without facts"
            );
        }
    });
}

#[test]
fn own_type_is_declared_or_imported_from_the_project() {
    let header = "import type { Order } from './order';
import type { Row } from '@/rows';
import type { Invoice } from '@acme/db/invoices';
import Def from './def';
import type { Request } from 'express';
import type { Stats } from 'node:fs';
import type * as models from './models';
import Legacy = require('./legacy');
import Lib = require('lib');
interface Local { a: string }
class Model {}
enum Kind { A = 'a' }
declare const model: Model;
type Generic<T> = T;
";
    // (type, own with `acme_facts`, own without facts)
    let cases = [
        ("Order", true, true),
        ("Row", true, false),
        ("Invoice", true, false),
        ("Def", true, true),
        ("models.Receipt", true, true),
        ("Legacy", true, true),
        ("Local", true, true),
        ("Model", true, true),
        ("Kind", true, true),
        ("Order['items'][number]", true, true),
        ("import('./x').Y", true, true),
        ("Order | string", true, true),
        ("Local & { b: 1 }", true, true),
        ("Request", false, false),
        ("Stats", false, false),
        ("Lib", false, false),
        ("Record<string, number>", false, false),
        ("Express.Request", false, false),
        ("keyof Local", false, false),
        ("typeof model", false, false),
        ("string", false, false),
        ("import('lodash').Z", false, false),
    ];
    let aliases: String = cases
        .iter()
        .enumerate()
        .map(|(i, (ty, _, _))| format!("type T{i} = {ty};\n"))
        .collect();
    let text = format!("{header}{aliases}");
    let acme = acme_facts();
    let none = no_facts();
    with_file(&text, |file| {
        for (i, (ty, own, own_without_facts)) in cases.iter().enumerate() {
            let alias = alias_type(file, &format!("T{i}"));
            assert_eq!(is_own_type(file, &acme, alias), *own, "{ty}");
            assert_eq!(
                is_own_type(file, &none, alias),
                *own_without_facts,
                "{ty} without facts"
            );
        }
        assert!(!is_own_type(file, &acme, alias_type(file, "Generic")), "T");
    });
}
