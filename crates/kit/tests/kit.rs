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
    with_file_at("src/a.ts", text, check);
}

/// The same for a file at `path`: `src/a.tsx` parses JSX.
fn with_file_at(path: &str, text: &str, check: impl FnOnce(&File<'_>)) {
    let allocator = Allocator::default();
    let source_type = SourceType::from_path(path).expect("a TS or JS path");
    let parsed = Parser::new(&allocator, text, source_type)
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
        path: Utf8Path::new(path),
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
        ("let probe = { a: 1 } as unknown;", false),
        (
            "const hidden = { a: 1 } as any;\nlet probe = hidden;",
            false,
        ),
        ("let probe = <object>{ a: 1 };", false),
        ("let probe = { a: 1 } as unknown as T;", false),
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
        ("const Row = 1; export interface Row {}", "Row"),
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
        (
            "export const Kind = { A: 'a' }; type Kind = typeof Kind;",
            false,
        ),
        ("const Row = 1; export interface Row {}", true),
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

#[test]
fn words_split_at_separators_case_steps_and_acronym_ends() {
    for (name, expected) in [
        ("userId", &["user", "Id"][..]),
        ("userID", &["user", "ID"]),
        ("loadHTTPURL", &["load", "HTTPURL"]),
        ("HTTPServer", &["HTTP", "Server"]),
        ("user_id", &["user", "id"]),
        ("MAX_RETRIES", &["MAX", "RETRIES"]),
        ("CACHE_E2E_TIMEOUT_MS", &["CACHE", "E2E", "TIMEOUT", "MS"]),
        ("order-item", &["order", "item"]),
        ("_value", &["value"]),
        ("IUser", &["I", "User"]),
        ("v2Api", &["v2", "Api"]),
        ("base64Encode", &["base64", "Encode"]),
        ("ERC20Token", &["ERC20", "Token"]),
        ("runE2ETimeoutMs", &["run", "E2E", "Timeout", "Ms"]),
        ("peerP2PLimit", &["peer", "P2P", "Limit"]),
        ("userIDs", &["user", "IDs"]),
        ("URLsMap", &["URLs", "Map"]),
        ("getXAxis", &["get", "X", "Axis"]),
        ("orders$", &["orders$"]),
        ("_", &[]),
    ] {
        assert_eq!(words(name), expected, "{name}");
    }
}

#[test]
fn case_matches_its_style_and_apply_rewrites_into_it() {
    for (case, good, bad) in [
        (
            Case::Kebab,
            &["order-item", "v2"][..],
            &["orderItem", "order_item", "-a", "a--b", "a-", ""][..],
        ),
        (
            Case::Camel,
            &["userId", "userID", "orders$", "$scope", "x"],
            &["UserId", "user_id"],
        ),
        (
            Case::Pascal,
            &["UserId", "HTTPServer", "T"],
            &["userId", "Order_Row"],
        ),
        (
            Case::Constant,
            &["MAX_RETRIES", "E2E", "A"],
            &["MAX__X", "MAX_", "Max"],
        ),
    ] {
        for name in good {
            assert!(case.matches(name), "{case:?} {name}");
        }
        for name in bad {
            assert!(!case.matches(name), "{case:?} {name:?}");
        }
    }

    for (case, name, expected) in [
        (Case::Camel, "user_id", "userId"),
        (Case::Camel, "MAX_RETRIES", "maxRetries"),
        (Case::Camel, "userID", "userId"),
        (Case::Camel, "userIDs", "userIds"),
        (Case::Pascal, "order_row", "OrderRow"),
        (Case::Pascal, "HTTPServer", "HttpServer"),
        (Case::Constant, "pageSize", "PAGE_SIZE"),
        (Case::Constant, "cacheE2eTimeoutMs", "CACHE_E2E_TIMEOUT_MS"),
        (Case::Constant, "runE2ETimeoutMs", "RUN_E2E_TIMEOUT_MS"),
        (Case::Kebab, "OrderItem", "order-item"),
    ] {
        assert_eq!(
            case.apply(name).as_deref(),
            Some(expected),
            "{case:?} {name}"
        );
    }
    for case in [Case::Kebab, Case::Camel, Case::Pascal, Case::Constant] {
        assert_eq!(case.apply("_"), None, "{case:?}");
    }
}

/// `text` after renaming its first symbol named `from` to `to`; `None` when `rename` refuses.
fn renamed(text: &str, from: &str, to: &str) -> Option<String> {
    renamed_at("src/a.ts", text, from, to)
}

fn renamed_at(path: &str, text: &str, from: &str, to: &str) -> Option<String> {
    let mut result = None;
    with_file_at(path, text, |file| {
        let scoping = file.semantic.scoping();
        let symbol = scoping
            .symbol_ids()
            .find(|&symbol| scoping.symbol_name(symbol) == from)
            .unwrap_or_else(|| panic!("no {from} in {text}"));
        result = rename(file, symbol, to).map(|mut edits| {
            edits.sort_by_key(|edit| edit.span.start);
            let mut out = text.to_string();
            for edit in edits.iter().rev() {
                out.replace_range(edit.span.start as usize..edit.span.end as usize, &edit.text);
            }
            out
        });
    });
    result
}

#[test]
fn rename_edits_every_declaration_and_reference_and_keeps_object_keys() {
    for (text, from, to, expected) in [
        (
            "const value = 1; const box = { value }; const { value: copy } = box; export { box, copy };",
            "value",
            "amount",
            "const amount = 1; const box = { value: amount }; const { value: copy } = box; export { box, copy };",
        ),
        (
            "import { source } from './s'; const { value } = source; use(value);",
            "value",
            "amount",
            "import { source } from './s'; const { value: amount } = source; use(amount);",
        ),
        (
            "import { source } from './s'; const { value = 1 } = source; use(value);",
            "value",
            "amount",
            "import { source } from './s'; const { value: amount = 1 } = source; use(amount);",
        ),
        (
            "import { source } from './s'; let value; ({ value } = source);",
            "value",
            "amount",
            "import { source } from './s'; let amount; ({ value: amount } = source);",
        ),
        (
            "import { source } from './s'; let value; ({ value = 2 } = source);",
            "value",
            "amount",
            "import { source } from './s'; let amount; ({ value: amount = 2 } = source);",
        ),
        (
            "import { source } from './s'; let value; const amt = 1; ({ value = amt } = source);",
            "amt",
            "fallback",
            "import { source } from './s'; let value; const fallback = 1; ({ value = fallback } = source);",
        ),
        (
            "const amount_ = 1; const box = { amount: amount_ }; export { box };",
            "amount_",
            "amount",
            "const amount = 1; const box = { amount }; export { box };",
        ),
        (
            "import { box } from './b'; const { amount: amt } = box; use(amt);",
            "amt",
            "amount",
            "import { box } from './b'; const { amount } = box; use(amount);",
        ),
        (
            "import { box } from './b'; let amt; ({ amount: amt = 1 } = box);",
            "amt",
            "amount",
            "import { box } from './b'; let amount; ({ amount = 1 } = box);",
        ),
        (
            "function over(a: string): void; function over(a: number): void; function over(a: unknown) {} over(1); export {};",
            "over",
            "run",
            "function run(a: string): void; function run(a: number): void; function run(a: unknown) {} run(1); export {};",
        ),
        (
            "interface IUser { a: 1 } interface IUser { b: 2 } let user: IUser; export {};",
            "IUser",
            "User",
            "interface User { a: 1 } interface User { b: 2 } let user: User; export {};",
        ),
        (
            "export function f(p: number, q: typeof p) { return q; }",
            "p",
            "size",
            "export function f(size: number, q: typeof size) { return q; }",
        ),
        (
            "function f_g() {} f_g(); export {};",
            "f_g",
            "fG",
            "function fG() {} fG(); export {};",
        ),
        (
            "class Order_row {} const row: Order_row = new Order_row(); export { row };",
            "Order_row",
            "OrderRow",
            "class OrderRow {} const row: OrderRow = new OrderRow(); export { row };",
        ),
        (
            "const text = 1; use(text); export {};",
            "text",
            "string",
            "const string = 1; use(string); export {};",
        ),
        (
            "export function isText(_v: unknown): _v is string { return typeof _v === 'string'; }",
            "_v",
            "value",
            "export function isText(value: unknown): value is string { return typeof value === 'string'; }",
        ),
        (
            "export type Check = (order_v: unknown) => asserts order_v;",
            "order_v",
            "orderV",
            "export type Check = (orderV: unknown) => asserts orderV;",
        ),
    ] {
        assert_eq!(renamed(text, from, to).as_deref(), Some(expected), "{text}");
    }
}

#[test]
fn rename_refuses_what_may_change_the_code_or_reach_past_the_file() {
    for (text, from, to) in [
        ("const a = 1; use(a); export {};", "a", "a"),
        ("const a = 1; use(a); export {};", "a", "1a"),
        ("const a = 1; use(a); export {};", "a", "a-b"),
        ("const a = 1; use(a); export {};", "a", "delete"),
        ("const a = 1; use(a); export {};", "a", "arguments"),
        ("type T = 1; let t: T; export {};", "T", "string"),
        ("export const a = 1;", "a", "b"),
        ("const a = 1; export { a };", "a", "b"),
        ("const a = 1; export default a;", "a", "b"),
        ("import { a } from './a'; use(a);", "a", "b"),
        ("declare const a: number; export {};", "a", "b"),
        (
            "declare global { interface Shape {} } export {};",
            "Shape",
            "Form",
        ),
        (
            "class C { constructor(private a: string) {} } new C('x'); export {};",
            "a",
            "b",
        ),
        ("@D() class A {} new A(); export {};", "A", "B"),
        ("const a = 1; a;", "a", "b"),
        ("const a = 1; eval('a'); export {};", "a", "b"),
        ("function f() {} export const list = [f];", "f", "g"),
        ("class A {} export const n = A.name;", "A", "B"),
        (
            "export const f = function named_fn() {};",
            "named_fn",
            "namedFn",
        ),
        (
            "function g(cb = () => {}) { return [cb]; } export { g };",
            "cb",
            "callback",
        ),
        (
            "import { x } from './x'; const { cb = () => {} } = x; export const l = [cb];",
            "cb",
            "callback",
        ),
        (
            "let f_g; f_g = () => {}; export const l = [f_g];",
            "f_g",
            "fG",
        ),
        (
            "class E_x extends Error { constructor() { super(); this.name = this.constructor.name; } } throw new E_x(); export {};",
            "E_x",
            "Ex",
        ),
        (
            "interface X<t_v> { a: t_v } interface X<t_v> { b: t_v } export {};",
            "t_v",
            "TV",
        ),
        (
            "const a = 1; function g() { const b = 2; return a + b; } export { g };",
            "a",
            "b",
        ),
        ("const a = 1; console.log(a, name); export {};", "a", "name"),
        (
            "export function f() { try { g(); } catch (e) { var error = e; } return error; }",
            "e",
            "error",
        ),
        (
            "const user_id = 1; const userID = 2; export {};",
            "user_id",
            "userId",
        ),
    ] {
        assert_eq!(renamed(text, from, to), None, "{text}: {from} -> {to}");
    }

    // A lowercase tag is an HTML element; a capital keeps it a component.
    let jsx = "import { icons } from './icons'; const Icon = icons.star; \
               export const view = () => <Icon></Icon>;";
    assert_eq!(renamed_at("src/a.tsx", jsx, "Icon", "icon"), None);
    assert!(renamed_at("src/a.tsx", jsx, "Icon", "Glyph").is_some());
}

#[test]
fn own_name_is_a_symbol_the_file_declares_for_itself() {
    let text = "import { imported } from './x';
declare const ambient: number;
declare function external(outside: string): void;
const variable = 1;
function func(parameter: number, ...rest: number[]) {}
class Klass { constructor(private property: string) {} }
try {} catch (caught) {}
interface Shape {}
type Alias = 1;
enum Color { Red }
function generic<T>() {}
declare global { interface GlobalShape {} }
namespace Space { const inner = 1; }";
    with_file(text, |file| {
        let scoping = file.semantic.scoping();
        let names = |own: bool| {
            let mut names: Vec<_> = scoping
                .symbol_ids()
                .filter(|&symbol| is_own_name(file, symbol) == own)
                .map(|symbol| scoping.symbol_name(symbol))
                .collect();
            names.sort_unstable();
            names
        };
        assert_eq!(
            names(true),
            [
                "Alias",
                "Color",
                "Klass",
                "Shape",
                "T",
                "caught",
                "func",
                "generic",
                "inner",
                "parameter",
                "rest",
                "variable",
            ]
        );
        assert_eq!(
            names(false),
            [
                "GlobalShape",
                "Red",
                "Space",
                "ambient",
                "external",
                "imported",
                "outside",
                "property",
            ]
        );
    });
}

#[test]
fn private_member_is_a_private_or_hash_member_with_a_body() {
    let text = "class A {
  private a = 1;
  #b = 2;
  private get c() { return 1; }
  private m(): void;
  private m() {}
  constructor(private d: string) {}
  e = 1;
  protected f = 1;
  public g() {}
  private ['h'] = 1;
  private declare z: number;
}
declare class B { private i: number; }";
    with_file(text, |file| {
        let members: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| {
                let member = private_member(file, node)?;
                assert!(PRIVATE_MEMBER_NODES.contains(&node.kind().ty()));
                Some(member)
            })
            .map(|(name, span)| (name, span.source_text(text)))
            .collect();
        assert_eq!(
            members,
            [("a", "a"), ("b", "#b"), ("c", "c"), ("m", "m"), ("d", "d")]
        );
    });
}

fn symbol_named(file: &File<'_>, name: &str) -> kit::oxc::semantic::SymbolId {
    let scoping = file.semantic.scoping();
    scoping
        .symbol_ids()
        .find(|&symbol| scoping.symbol_name(symbol) == name)
        .unwrap_or_else(|| panic!("no {name}"))
}

#[test]
fn related_symbols_share_a_line_of_scopes_and_similar_names_differ_in_separators_and_case() {
    assert!(is_similar_name("user_id", "userID"));
    assert!(is_similar_name("$user-id", "UserId"));
    assert!(!is_similar_name("userIds", "userId"));

    let text = "const outer = 1;
function f(param: number) { const inner = param; return () => { const deep = inner; return deep; }; }
function g() { const other = outer; return other; }
export { f, g };";
    with_file(text, |file| {
        let scoping = file.semantic.scoping();
        let mut related: Vec<_> = related_symbols(file, symbol_named(file, "inner"))
            .map(|symbol| scoping.symbol_name(symbol))
            .collect();
        related.sort_unstable();
        assert_eq!(related, ["deep", "f", "g", "outer", "param"]);
    });
}

#[test]
fn jsx_tag_script_global_and_ambient_name_where_a_symbol_or_node_stands() {
    let text = "import { icons } from './i'; const Icon = icons.a; const label = 'x'; \
                export const view = <Icon>{label}</Icon>;";
    with_file_at("src/a.tsx", text, |file| {
        assert!(is_jsx_tag(file, symbol_named(file, "Icon")));
        assert!(!is_jsx_tag(file, symbol_named(file, "label")));
    });

    with_file(
        "function run() { const inner = 1; return inner; } run();",
        |file| {
            assert!(is_script_global(file, symbol_named(file, "run")));
            assert!(!is_script_global(file, symbol_named(file, "inner")));
        },
    );
    with_file("function run() {} run(); export {};", |file| {
        assert!(!is_script_global(file, symbol_named(file, "run")));
    });

    let text = "declare module 'm' { class A {} }
declare namespace N { class B {} }
declare class C {}
class D {}
export { D };";
    with_file(text, |file| {
        let ambient: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::Class(class) => Some((
                    class.id.as_ref().map_or("", |id| id.name.as_str()),
                    is_ambient(file, node.id()),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(
            ambient,
            [("A", true), ("B", true), ("C", true), ("D", false)]
        );
    });
}

#[test]
fn global_value_nullable_type_and_outer_assertion_read_through_the_syntax() {
    let text = "const Map = 1; new Proxy({}, {}); new Map(); export {};";
    with_file(text, |file| {
        let globals: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::NewExpression(new) => Some((
                    is_global(file, &new.callee, "Proxy"),
                    is_global(file, &new.callee, "Map"),
                )),
                _ => None,
            })
            .collect();
        assert_eq!(globals, [(true, false), (false, false)]);
    });

    let text = "type A = boolean | null | undefined; type B = boolean | string; type C = null; \
                type D = boolean;";
    with_file(text, |file| {
        let is_boolean = |ty: &TSType<'_>| matches!(ty, TSType::TSBooleanKeyword(_));
        for (name, expected) in [("A", true), ("B", false), ("C", false), ("D", true)] {
            assert_eq!(
                is_nullable(alias_type(file, name), is_boolean),
                expected,
                "{name}"
            );
        }
    });

    let text = "declare const x: { a: number }; (x as { a: number })!.a = 1; x.a = 2;";
    with_file(text, |file| {
        let outer: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter(
                |node| matches!(node.kind(), AstKind::IdentifierReference(id) if id.name == "x"),
            )
            .map(|node| outer_assertion(file, node).kind().ty())
            .collect();
        assert_eq!(
            outer,
            [AstType::TSNonNullExpression, AstType::IdentifierReference]
        );
    });
}

#[test]
fn path_span_is_empty_where_the_code_starts() {
    for (text, start) in [
        ("'use strict';\nconst a = 1;", 0),
        ("// note\nconst a = 1;", 8),
        ("// only a comment", 0),
        ("", 0),
    ] {
        with_file(text, |file| {
            assert_eq!(path_span(file), Span::empty(start), "{text:?}");
        });
    }
}

#[test]
fn start_after_decorators_skips_the_decorators_and_the_comments_after_them() {
    let text = "declare function D(): PropertyDecorator;
class A {
  @D() /* note */ x = 1;
  @D()
  // note
  y = 2;
  z = 3;
}
export { A };";
    with_file(text, |file| {
        let starts: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::PropertyDefinition(property) => {
                    let start = start_after_decorators(file, property.span, &property.decorators);
                    Some(&text[start as usize..property.span.end as usize])
                }
                _ => None,
            })
            .collect();
        assert_eq!(starts, ["x = 1;", "y = 2;", "z = 3;"]);
    });
}

#[test]
fn callback_is_a_function_handed_to_a_call_new_jsx_or_an_object() {
    let text = "declare function run(f: unknown): void;
run(() => 1);
const kept = () => 2;
const handlers = { pick: () => 3 };
new Promise(function () {});
export const view = <button onClick={() => 4} />;
export { kept, handlers };";
    with_file_at("src/a.tsx", text, |file| {
        let callbacks: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter(|node| match node.kind() {
                AstKind::ArrowFunctionExpression(_) => true,
                AstKind::Function(function) => function.body.is_some(),
                _ => false,
            })
            .map(|node| is_callback(file, node))
            .collect();
        assert_eq!(callbacks, [true, false, true, true, true]);
    });
}

#[test]
fn member_use_names_what_a_member_access_or_a_destructured_key_reaches() {
    let text = "class A {
  #p = 1;
  q = 2;
  m(key: 'q'): unknown[] {
    const { q } = this;
    let r;
    ({ q: r } = this);
    return [this.#p, this.q, this['q'], this[key], (this as A)[key], q, r, [1][0]];
  }
}
export { A };";
    with_file(text, |file| {
        let uses: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| member_use(file, node))
            .collect();
        let name = |name: &'static str| MemberUse::Name(name.into());
        assert_eq!(
            uses,
            [
                name("q"),
                name("q"),
                name("#p"),
                name("q"),
                name("q"),
                MemberUse::Any,
                MemberUse::Any,
            ]
        );
    });
}

#[test]
fn top_classes_are_class_declarations_and_whole_class_values_of_variables() {
    let text = "declare function Dec(): ClassDecorator;
declare function mixin<T>(base: T): T;
@Dec() export class A {}
export abstract class B {}
export default class {}
export const C = class {};
const D = class Named {};
declare class E {}
export function f() { return class Inner {}; }
const G = mixin(class {});
export const list = [class {}];
let L = class {};
export @Dec() /* class */ class H {}";
    with_file(text, |file| {
        let classes: Vec<_> = top_classes(file)
            .iter()
            .map(|class| (class.name, class.head.source_text(text)))
            .collect();
        assert_eq!(
            classes,
            [
                ("A", "class A"),
                ("B", "abstract class B"),
                ("default", "class"),
                ("C", "C"),
                ("Named", "class Named"),
                ("H", "class H"),
            ]
        );
    });

    let text = "export default abstract /* class */ class {}";
    with_file(text, |file| {
        let heads: Vec<_> = top_classes(file)
            .iter()
            .map(|class| class.head.source_text(text))
            .collect();
        assert_eq!(heads, ["abstract /* class */ class"]);
    });
}

#[test]
fn top_bindings_are_functions_with_a_body_and_variables_outside_declare_and_blocks() {
    let text = "function f() {}
function g(a: string): void; function g(a: unknown): void {}
declare function h(): void;
export const i = () => 1;
const j = (function () {}) as unknown;
export default function () {}
const K = 1, { l, m } = { l: 1, m: 2 };
let n = 1;
var o;
let p = () => 1;
const Q = class {};
declare const r: number;
namespace S { export const t = 1; }
if (n) { const u = 1; }";
    with_file(text, |file| {
        let bindings: Vec<_> = top_bindings(file)
            .iter()
            .map(|binding| {
                (
                    binding.span.source_text(text),
                    binding.kind,
                    binding.symbols.len(),
                    binding.init.is_some(),
                )
            })
            .collect();
        assert_eq!(
            bindings,
            [
                ("f", TopKind::Function, 1, false),
                ("g", TopKind::Function, 1, false),
                ("i", TopKind::Function, 1, true),
                ("j", TopKind::Function, 1, true),
                ("function ()", TopKind::Function, 0, false),
                ("K", TopKind::Constant, 1, true),
                ("{ l, m }", TopKind::Constant, 2, true),
                ("n", TopKind::Variable, 1, true),
                ("o", TopKind::Variable, 1, false),
                ("p", TopKind::Variable, 1, true),
            ]
        );
    });

    with_file("export default (function named() {});", |file| {
        let bindings = top_bindings(file);
        assert_eq!(bindings.len(), 1);
        assert!(bindings[0].symbols.is_empty());
    });
}
