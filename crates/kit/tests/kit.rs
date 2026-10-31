use std::collections::BTreeSet;

use camino::Utf8Path;
use kit::oxc::ast::ast::{Expression, TSType};
use kit::oxc::parser::ParseOptions;
use kit::oxc::semantic::{NodeId, SemanticBuilder};
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
            ("#db/client", ImportKind::Alias),
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
            ("#db/client", ImportKind::Alias),
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
fn global_member_is_a_static_member_of_a_global_value() {
    let text = "import { Reflect as R } from 'reflect';
const JSON = { parse: (text: string) => text };
export const calls = [Reflect.get(a, 'k'), Reflect['apply'](f, a, []), Reflect?.get(a, 'k'),
  (Reflect as any).get(a, 'k'), (Object.freeze)({}), Reflect[key](a), R.get(a, 'k'),
  JSON.parse('1'), get(a), (Reflect.get as typeof Reflect.get)(a, 'k')];";
    with_file(text, |file| {
        let members: Vec<Option<(&str, &str)>> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::CallExpression(call) => Some(global_member(file, &call.callee)),
                _ => None,
            })
            .collect();
        assert_eq!(
            members,
            [
                Some(("Reflect", "get")),
                Some(("Reflect", "apply")),
                Some(("Reflect", "get")),
                Some(("Reflect", "get")),
                Some(("Object", "freeze")),
                None,
                None,
                None,
                None,
                Some(("Reflect", "get")),
            ]
        );
    });
}

#[test]
fn declared_is_the_annotation_or_the_value_of_a_const() {
    let text = "export function f(typed: number[], untyped, { part }: { part: number[] }, ...rest: string[]) {
  const value = [1];
  const marked: string[] = [];
  let open = [1];
  let typedLet: number[] = [];
  const { taken } = { taken: [1] };
  return [typed, untyped, part, rest, value, marked, open, typedLet, taken, f, Math];
}";
    with_file(text, |file| {
        let declared: Vec<Option<String>> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::ArrayExpression(array) if array.elements.len() > 2 => Some(array),
                _ => None,
            })
            .flat_map(|array| array.elements.iter().filter_map(|e| e.as_expression()))
            .map(|element| {
                let Expression::Identifier(id) = element else {
                    return None;
                };
                declared(file, id).map(|(_, declared)| match declared {
                    Declared::Annotation(ty) => format!("type {}", source(file, ty)),
                    Declared::Const(value) => format!("const {}", source(file, value)),
                })
            })
            .collect();
        let expected = [
            Some("type number[]"),
            None,
            None,
            Some("type string[]"),
            Some("const [1]"),
            Some("type string[]"),
            None,
            Some("type number[]"),
            None,
            None,
            None,
        ];
        assert_eq!(declared, expected.map(|text| text.map(String::from)));
    });
}

#[test]
fn global_references_are_the_references_no_declaration_or_import_holds() {
    let text = "import { log } from './log';
console.log(1);
function f(console: { log(x: number): void }) { console.log(2); }
type C = typeof console;
log(console);
{ const console = { log }; console.log(3); }
export {};";
    with_file(text, |file| {
        let found = |name: &str| {
            global_references(file, name)
                .map(|node| {
                    let parent = file.semantic.nodes().parent_node(node.id());
                    (node.kind().ty(), source(file, parent))
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(
            found("console"),
            [
                (AstType::IdentifierReference, "console.log"),
                (AstType::IdentifierReference, "typeof console"),
                (AstType::IdentifierReference, "log(console)"),
            ]
        );
        assert_eq!(found("log"), []);
        assert_eq!(found("process"), []);
    });
}

#[test]
fn a_path_report_is_empty_where_the_code_starts() {
    let facts = no_facts();
    for (text, start) in [
        ("'use strict';\nconst a = 1;", 0),
        ("// note\nconst a = 1;", 8),
        ("// only a comment", 0),
        ("", 0),
    ] {
        let mut out = Vec::new();
        with_file(text, |file| {
            Cx::new(file, &facts, &mut out).report_path();
        });
        assert_eq!(out[0].span, Span::empty(start), "{text:?}");
        assert!(out[0].on_path);
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
fn class_file_classes_are_top_classes_outside_functions_and_constants_files() {
    let text = "export class A {}\nexport const b = 1;";
    with_file(text, |file| {
        let names = |role: Option<Role>| -> Vec<&str> {
            let file = File { role, ..*file };
            class_file_classes(&file)
                .iter()
                .map(|class| class.name)
                .collect()
        };
        assert_eq!(names(None), ["A"]);
        assert_eq!(names(Some(Role::Types)), ["A"]);
        assert_eq!(names(Some(Role::Functions)), Vec::<&str>::new());
        assert_eq!(names(Some(Role::Constants)), Vec::<&str>::new());
    });
    with_file("export const b = 1;", |file| {
        assert!(class_file_classes(file).is_empty());
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

/// `text` after importing `names` from `source` with `acme_facts`; `None` when `add_import`
/// refuses.
fn imported(text: &str, source: &str, names: &[&str]) -> Option<String> {
    let acme = acme_facts();
    let mut result = None;
    with_file(text, |file| {
        result = add_import(file, &acme, source, names).map(|mut edits| {
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
fn add_import_merges_into_a_value_import_of_the_module() {
    for (text, source, names, expected) in [
        (
            "import { a } from 'node:perf_hooks';\nexport const x = a;\n",
            "node:perf_hooks",
            &["performance"][..],
            "import { a, performance } from 'node:perf_hooks';\nexport const x = a;\n",
        ),
        // The same builtin; `node-protocol` adds `node:`.
        (
            "import { a } from 'perf_hooks';\nexport const x = a;\n",
            "node:perf_hooks",
            &["performance"],
            "import { a, performance } from 'perf_hooks';\nexport const x = a;\n",
        ),
        (
            "import ph from 'node:perf_hooks';\nexport const x = ph;\n",
            "node:perf_hooks",
            &["performance"],
            "import ph, { performance } from 'node:perf_hooks';\nexport const x = ph;\n",
        ),
        (
            "import { Controller, HttpStatus } from '@nestjs/common';\nexport const x = [Controller, HttpStatus];\n",
            "@nestjs/common",
            &["HttpCode", "HttpStatus"],
            "import { Controller, HttpStatus, HttpCode } from '@nestjs/common';\nexport const x = [Controller, HttpStatus];\n",
        ),
        (
            "import { Controller } from '@nestjs/common';\nexport const x = Controller;\n",
            "@nestjs/common",
            &["HttpCode", "HttpCode"],
            "import { Controller, HttpCode } from '@nestjs/common';\nexport const x = Controller;\n",
        ),
        // Without a trailing comma; the comment stays with `Get`.
        (
            "import {\n  Controller,\n  Get // legacy\n} from '@nestjs/common';\nexport const x = [Controller, Get];\n",
            "@nestjs/common",
            &["HttpCode", "HttpStatus"],
            "import {\n  Controller,\n  Get, // legacy\n  HttpCode,\n  HttpStatus\n} from '@nestjs/common';\nexport const x = [Controller, Get];\n",
        ),
        (
            "import {\n  Controller,\n  Get,\n} from '@nestjs/common';\nexport const x = [Controller, Get];\n",
            "@nestjs/common",
            &["HttpCode"],
            "import {\n  Controller,\n  Get,\n  HttpCode,\n} from '@nestjs/common';\nexport const x = [Controller, Get];\n",
        ),
        // A `}` in a comment does not close the list.
        (
            "import {\n  monitorEventLoopDelay,\n  // see {@link x}\n} from 'node:perf_hooks';\nexport const x = monitorEventLoopDelay;\n",
            "node:perf_hooks",
            &["performance"],
            "import {\n  monitorEventLoopDelay,\n  // see {@link x}\n  performance,\n} from 'node:perf_hooks';\nexport const x = monitorEventLoopDelay;\n",
        ),
    ] {
        assert_eq!(
            imported(text, source, names).as_deref(),
            Some(expected),
            "{text}"
        );
    }
}

#[test]
fn add_import_puts_a_new_line_in_the_last_block_by_group() {
    for (text, source, names, expected) in [
        // `import * as` and `import type` take no names.
        (
            "import * as ph from 'node:perf_hooks';\nexport const x = ph;\n",
            "node:perf_hooks",
            &["performance"][..],
            "import * as ph from 'node:perf_hooks';\nimport { performance } from 'node:perf_hooks';\nexport const x = ph;\n",
        ),
        (
            "import type { EventLoopUtilization } from 'node:perf_hooks';\nexport type U = EventLoopUtilization;\n",
            "node:perf_hooks",
            &["performance"],
            "import type { EventLoopUtilization } from 'node:perf_hooks';\nimport { performance } from 'node:perf_hooks';\nexport type U = EventLoopUtilization;\n",
        ),
        (
            "import { readFile } from 'node:fs/promises';\nimport { z } from 'zod';\nexport const x = [readFile, z];\n",
            "node:perf_hooks",
            &["performance"],
            "import { readFile } from 'node:fs/promises';\nimport { performance } from 'node:perf_hooks';\nimport { z } from 'zod';\nexport const x = [readFile, z];\n",
        ),
        // A block spaced by blank lines gets one between groups.
        (
            "import { z } from 'zod';\n\nimport { tax } from './tax';\nexport const x = [tax, z];\n",
            "node:perf_hooks",
            &["performance"],
            "import { performance } from 'node:perf_hooks';\n\nimport { z } from 'zod';\n\nimport { tax } from './tax';\nexport const x = [tax, z];\n",
        ),
        (
            "import { z } from 'zod';\nimport { tax } from './tax';\nexport const x = [tax, z];\n",
            "node:perf_hooks",
            &["performance"],
            "import { performance } from 'node:perf_hooks';\nimport { z } from 'zod';\nimport { tax } from './tax';\nexport const x = [tax, z];\n",
        ),
        (
            "import { readFile } from 'node:fs/promises';\n\nimport { tax } from './tax';\nexport const x = [tax, readFile];\n",
            "zod",
            &["z"],
            "import { readFile } from 'node:fs/promises';\n\nimport { z } from 'zod';\n\nimport { tax } from './tax';\nexport const x = [tax, readFile];\n",
        ),
        // The block is `./tax` alone: `reflect-metadata` keeps its place.
        (
            "import 'reflect-metadata';\n\nimport { tax } from './tax';\nexport const x = tax;\n",
            "@nestjs/common",
            &["Injectable"],
            "import 'reflect-metadata';\n\nimport { Injectable } from '@nestjs/common';\nimport { tax } from './tax';\nexport const x = tax;\n",
        ),
        // `./metrics` right under `./telemetry` loads in that order: the block is `zod` alone.
        (
            "import './telemetry';\nimport { metrics } from './metrics';\n\nimport { z } from 'zod';\nexport const x = [metrics, z];\n",
            "node:perf_hooks",
            &["performance"],
            "import './telemetry';\nimport { metrics } from './metrics';\n\nimport { performance } from 'node:perf_hooks';\nimport { z } from 'zod';\nexport const x = [metrics, z];\n",
        ),
        (
            "// biome-ignore lint/style/useImportType: legacy\nimport { z } from 'zod';\nexport const x = z;\n",
            "node:perf_hooks",
            &["performance"],
            "import { performance } from 'node:perf_hooks';\n// biome-ignore lint/style/useImportType: legacy\nimport { z } from 'zod';\nexport const x = z;\n",
        ),
        // A second list after the default would not parse.
        (
            "import ph, {} from 'node:perf_hooks';\nexport const x = ph;\n",
            "node:perf_hooks",
            &["performance"],
            "import ph, {} from 'node:perf_hooks';\nimport { performance } from 'node:perf_hooks';\nexport const x = ph;\n",
        ),
        // The engine turns the `\n` of an edit into the file's line ending.
        (
            "import { z } from 'zod';\r\n\r\nimport { tax } from './tax';\r\nexport const x = [tax, z];\r\n",
            "node:perf_hooks",
            &["performance"],
            "import { performance } from 'node:perf_hooks';\n\nimport { z } from 'zod';\r\n\r\nimport { tax } from './tax';\r\nexport const x = [tax, z];\r\n",
        ),
        (
            "// eslint-disable-next-line no-restricted-imports\nimport { z } from 'zod';\nexport const x = z;\n",
            "node:perf_hooks",
            &["performance"],
            "import { performance } from 'node:perf_hooks';\n// eslint-disable-next-line no-restricted-imports\nimport { z } from 'zod';\nexport const x = z;\n",
        ),
        (
            "import { z } from \"zod\"\nexport const x = z\n",
            "node:perf_hooks",
            &["performance"],
            "import { performance } from \"node:perf_hooks\"\nimport { z } from \"zod\"\nexport const x = z\n",
        ),
        // Workspace and alias are one group.
        (
            "import { z } from 'zod';\n\nimport { db } from '@acme/db';\n\nimport { tax } from './tax';\nexport const x = [z, db, tax];\n",
            "@/clock",
            &["Clock"],
            "import { z } from 'zod';\n\nimport { db } from '@acme/db';\nimport { Clock } from '@/clock';\n\nimport { tax } from './tax';\nexport const x = [z, db, tax];\n",
        ),
    ] {
        assert_eq!(
            imported(text, source, names).as_deref(),
            Some(expected),
            "{text}"
        );
    }
}

#[test]
fn add_import_without_a_block_goes_after_the_imports_or_above_the_code() {
    for (text, expected) in [
        (
            "import 'reflect-metadata';\n\nexport const a = 1;\n",
            "import 'reflect-metadata';\nimport { performance } from 'node:perf_hooks';\n\nexport const a = 1;\n",
        ),
        // `./tax` right under `reflect-metadata` keeps the load order, so the new module loads
        // after both.
        (
            "import 'reflect-metadata';\nimport { tax } from './tax';\n\nexport const a = tax;\n",
            "import 'reflect-metadata';\nimport { tax } from './tax';\nimport { performance } from 'node:perf_hooks';\n\nexport const a = tax;\n",
        ),
        // The doc comment stays with its function; the header stays on top.
        (
            "// Copyright.\n\n/** Doc. */\nexport function f(): void {}\n",
            "// Copyright.\n\nimport { performance } from 'node:perf_hooks';\n\n/** Doc. */\nexport function f(): void {}\n",
        ),
        // TypeScript reads a pragma only above the first statement.
        (
            "// @ts-nocheck\nexport const a = 1;\n",
            "// @ts-nocheck\nimport { performance } from 'node:perf_hooks';\n\nexport const a = 1;\n",
        ),
    ] {
        assert_eq!(
            imported(text, "node:perf_hooks", &["performance"]).as_deref(),
            Some(expected),
            "{text}"
        );
    }
}

#[test]
fn add_import_refuses_taken_names_and_scripts_and_skips_imported_ones() {
    let text = "import { performance } from 'node:perf_hooks';\nexport const x = performance;\n";
    assert_eq!(
        imported(text, "node:perf_hooks", &["performance"]).as_deref(),
        Some(text)
    );

    for text in [
        "const performance = 1;\nexport { performance };\n",
        "import { performance } from './perf';\nexport const x = performance;\n",
        // A script: the import would make its global names local.
        "const x = 1;\n",
    ] {
        assert_eq!(
            imported(text, "node:perf_hooks", &["performance"]),
            None,
            "{text}"
        );
    }
    for name in ["default", "not-a-name"] {
        assert_eq!(
            imported(
                "import { z } from 'zod';\nexport const x = z;\n",
                "zod",
                &[name]
            ),
            None,
            "{name}"
        );
    }
}

/// The answers of `answer` for every function with a body, in source order.
fn per_function(file: &File<'_>, answer: impl Fn(&AstNode<'_>) -> bool) -> Vec<bool> {
    file.semantic
        .nodes()
        .iter()
        .filter(|node| match node.kind() {
            AstKind::ArrowFunctionExpression(_) => true,
            AstKind::Function(function) => function.body.is_some(),
            _ => false,
        })
        .map(answer)
        .collect()
}

#[test]
fn signature_is_given_through_branches_fallbacks_arrays_and_accessors() {
    let text = "type Pick = (a: number, b: number) => number;
interface Store { save(key: string): void }
declare const flag: boolean;
declare const custom: Pick | undefined;
const one: Pick = flag ? (a, b) => a + b : (a, b) => a - b;
const two: Pick = custom ?? ((a, b) => a * b);
const three: Pick[] = [(a, b) => a / b];
const free = flag ? (a: number) => a : (a: number) => -a;
class Box implements Store {
  accessor pick: Pick = (a, b) => a % b;
  save(key: string): void { void key; }
  private hide(key: string): void { void key; }
}
export { one, two, three, free, Box };";
    with_file(text, |file| {
        assert_eq!(
            per_function(file, |node| is_signature_given(file, node)),
            [true, true, true, true, false, false, true, true, false]
        );
    });
}

#[test]
fn di_constructor_is_the_constructor_of_a_decorated_class() {
    let text = "declare function Injectable(): ClassDecorator;
@Injectable()
export class A { constructor(x: number) { void x; } }
export class B { constructor(x: number) { void x; } }";
    with_file(text, |file| {
        assert_eq!(
            per_function(file, |node| is_di_constructor(file, node)),
            [true, false]
        );
    });
    let text = "export class C { a() {} private b() {} protected c() {} #d() {} }";
    with_file(text, |file| {
        let public: Vec<bool> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::MethodDefinition(method) => {
                    Some(is_public(method.accessibility, &method.key))
                }
                _ => None,
            })
            .collect();
        assert_eq!(public, [true, false, false, false]);
    });
}

#[test]
fn local_callee_is_the_function_of_this_file_a_call_runs() {
    let text = "function f() { return 1; }
const g = () => 2;
let h = () => 3;
h = () => 4;
declare function d(): number;
export class K {
  m() { return 5; }
  n = () => 6;
  static s() { return 7; }
  static t(k: K) { return k.m(); }
  run() {
    function inner(this: K) { return this.m(); }
    return [f(), g(), h(), d(), this.m(), this.n(), K.s(), (() => this.m())(), inner.call(this)];
  }
}";
    with_file(text, |file| {
        let callees: Vec<Option<&str>> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::CallExpression(call) => Some(
                    local_callee(file, call)
                        .map(|callee| source(file, callee).lines().next().unwrap_or_default()),
                ),
                _ => None,
            })
            .collect();
        assert_eq!(
            callees,
            [
                // `k.m()` in a static method, `this.m()` in a function of its own.
                None,
                None,
                Some("function f() { return 1; }"),
                Some("() => 2"),
                None,
                None,
                Some("() { return 5; }"),
                Some("() => 6"),
                None,
                None,
                Some("() { return 5; }"),
                None,
            ]
        );
    });

    let text = "function o(id: string): number;
function o(id: number): number;
function o(id: string | number): number { return Number(id); }
export class P {
  #m() { return 1; }
  m() { return 2; }
  #n = () => 3;
  run() { return [o(1), this.#m(), this.m(), this.#n()]; }
}";
    with_file(text, |file| {
        let callees: Vec<Option<&str>> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::CallExpression(call) => {
                    Some(local_callee(file, call).map(|callee| source(file, callee)))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            callees,
            [
                None,
                // An overload runs the declaration with a body; `#m` and `m` are two members.
                Some("function o(id: string | number): number { return Number(id); }"),
                Some("() { return 1; }"),
                Some("() { return 2; }"),
                Some("() => 3"),
            ]
        );
    });
}

#[test]
fn returns_promise_is_async_or_a_lib_promise_return_type() {
    let text = "declare const later: Promise<number>;
async function a() {}
function b(): Promise<number> { return later; }
function c() { return later; }
async function* d() {}
const e = async () => 1;
const f = (): Promise<number> => later;
const g = () => later;
export class K {
  async m() {}
  n(): Promise<number> { return later; }
  o() {}
}
export function shadowed() {
  type Promise<T> = { value: T };
  function h(): Promise<number> { return { value: 1 }; }
  return h;
}
export { a, b, c, d, e, f, g };";
    with_file(text, |file| {
        // `c` and `g` give a promise without saying so: only the syntax counts.
        assert_eq!(
            per_function(file, |node| returns_promise(file, node)),
            [
                true, true, false, false, true, true, false, true, true, false, false, false
            ]
        );
    });
}

#[test]
fn call_of_is_a_call_also_through_optional_chaining_and_assertions() {
    let text = "declare const client: { send(): Promise<void> } | undefined;
declare function load(): unknown;
declare const save: (() => void) | undefined;
load();
client?.send();
save?.();
load() as Promise<void>;
load()!;
client?.send;
void load();
new Promise(() => {});";
    with_file(text, |file| {
        let calls: Vec<Option<&str>> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::ExpressionStatement(statement) => {
                    Some(call_of(&statement.expression).map(|call| source(file, call)))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            calls,
            [
                Some("load()"),
                Some("client?.send()"),
                Some("save?.()"),
                Some("load()"),
                Some("load()"),
                None,
                None,
                None,
            ]
        );
    });
}

#[test]
fn if_runs_are_the_tests_of_adjacent_ifs_and_their_else_ifs() {
    let text = "export function f(a: number) {
  if (a === 1) {} else if (a === 2) {}
  if (a === 3) {}
  const b = a;
  if (b === 4) {} else { if (b === 5) {} }
}";
    with_file(text, |file| {
        let body = file
            .semantic
            .nodes()
            .iter()
            .find(|node| matches!(node.kind(), AstKind::FunctionBody(_)))
            .unwrap();
        let runs: Vec<Vec<&str>> = if_runs(body.kind())
            .map(|tests| tests.iter().map(|test| source(file, *test)).collect())
            .collect();
        assert_eq!(
            runs,
            [vec!["a === 1", "a === 2", "a === 3"], vec!["b === 4"]]
        );
    });
}

#[test]
fn option_key_is_optional_in_the_visible_type_or_the_parameter_is_optional() {
    let text = "interface Opts { retries?: number; label: string | null }
export function f(a: Opts, b: Readonly<Opts>, c: Partial<{ x: number }>, g: Ext, d?: Ext, e: Ext = {}) {
  return [a, b, c, g, d, e];
}";
    with_file(text, |file| {
        let parameters: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::FormalParameter(parameter) => Some(parameter),
                _ => None,
            })
            .collect();
        let keys: Vec<bool> = [
            (0, "retries"),
            (0, "label"),
            (1, "retries"),
            (2, "x"),
            (3, "any"),
            (4, "any"),
            (5, "any"),
        ]
        .into_iter()
        .map(|(index, key)| is_option_key(file, parameters[index], key))
        .collect();
        assert_eq!(keys, [true, false, true, true, false, true, true]);
    });
}

#[test]
fn member_write_and_mutator_call_find_what_changes_a_parameter() {
    let text = "export function f(items: string[], o: { a: number; b?: string }, pair: [number, string], src: Record<string, number>) {
  items.push('x');
  items.map(String);
  o.a = 1;
  void o.a;
  for (o.a of [1]) {}
  [o.a, o.b] = pair;
  ({ b: o.b = '' } = {});
  ({ [o.b ?? '']: o.a } = src);
  delete o?.b;
}";
    with_file(text, |file| {
        let scoping = file.semantic.scoping();
        let changes = |name: &str, find: &dyn Fn(kit::oxc::semantic::NodeId) -> Option<Span>| {
            scoping
                .get_resolved_references(symbol_named(file, name))
                .map(|reference| find(reference.node_id()).map(|span| span.source_text(file.text)))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            changes("items", &|id| mutator_call(file, id)),
            [Some("items.push('x')"), None]
        );
        assert_eq!(
            changes("o", &|id| member_write(file, id)),
            [
                Some("o.a = 1"),
                None,
                Some("o.a"),
                Some("[o.a, o.b] = pair"),
                Some("[o.a, o.b] = pair"),
                Some("{ b: o.b = '' } = {}"),
                // The computed key is read.
                None,
                Some("{ [o.b ?? '']: o.a } = src"),
                Some("delete o?.b"),
            ]
        );
    });
}

#[test]
fn log_call_is_a_level_method_on_a_receiver_with_a_logger_word() {
    let text = "declare const logger: any, Logger: any, console: any, dialog: any, metrics: any;
declare const remoteLogger: any, log: any, catalog: any;
class A {
  logger = logger;
  #log = logger;
  run() {
    console.error(1);
    this.logger.warn(2);
    this.#log.info(3);
    Logger.log(4);
    logger?.error(5);
    (remoteLogger as any).debug(6);
    logger['warn'](7);
    log(8);
    dialog.error(9);
    metrics.error(10);
    catalog.info(11);
    logger.flush(12);
    logger.child({}).info(13);
  }
}
export {};";
    let loggers = ["console", "log", "logger"].map(String::from);
    let levels = ["log", "debug", "info", "warn", "error"].map(String::from);
    with_file(text, |file| {
        let logs: Vec<_> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::CallExpression(call) if is_log_call(call, &loggers, &levels) => {
                    Some(source(file, call))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            logs,
            [
                "console.error(1)",
                "this.logger.warn(2)",
                "this.#log.info(3)",
                "Logger.log(4)",
                "logger?.error(5)",
                "(remoteLogger as any).debug(6)",
            ]
        );
    });
}

#[test]
fn import_blocks_leave_out_the_load_order_under_a_side_effect_import() {
    let text = "import 'reflect-metadata';
import { a } from './a';
import { b } from './b';
import { z } from 'zod';
import './x';

import { c } from './c';
import type { T } from './t';
export const k = 1;
import { d } from 'node:fs';
import './y';
import type { U } from './u';
import { e } from 'e';
export const all = [a, b, z, c, d, e] as unknown as [T, U];";
    let acme = acme_facts();
    with_file(text, |file| {
        let blocks: Vec<Vec<&str>> = import_blocks(file, &acme)
            .iter()
            .map(|block| {
                block
                    .iter()
                    .map(|import| import.source.value.as_str())
                    .collect()
            })
            .collect();
        assert_eq!(
            blocks,
            [
                vec!["zod"],
                vec!["./c", "./t"],
                vec!["node:fs"],
                vec!["./u", "e"]
            ]
        );
        let groups: Vec<u8> = ["node:fs", "fs", "zod", "@acme/db", "@/x", "./c"]
            .into_iter()
            .map(|source| import_group(file, &acme, source))
            .collect();
        assert_eq!(groups, [0, 0, 1, 2, 2, 3]);
    });
}

#[test]
fn module_source_package_name_and_own_kind() {
    let text = "import a from 'a';
export { b } from 'b';
export * from 'c';
const d = await import('d');
type E = import('e').E;
import f = require('f');
const g = require('g');
export const all = [a, d, f, g] as unknown as E;";
    with_file(text, |file| {
        let sources: Vec<&str> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| module_source(file, node.kind()))
            .map(|source| source.value.as_str())
            .collect();
        assert_eq!(sources, ["a", "b", "c", "d", "e", "f", "g"]);
    });
    for (source, name) in [
        ("@scope/name/sub", "@scope/name"),
        ("name/sub", "name"),
        ("name", "name"),
    ] {
        assert_eq!(package_name(source), name, "{source}");
    }
    for (kind, own) in [
        (ImportKind::Node, false),
        (ImportKind::External, false),
        (ImportKind::Workspace, true),
        (ImportKind::Alias, true),
        (ImportKind::Relative, true),
    ] {
        assert_eq!(kind.is_own(), own, "{kind:?}");
    }
}

#[test]
fn imported_name_is_the_module_and_the_exported_name_of_an_import() {
    let text = "import { Get, Post as Create } from '@nestjs/common';
import Joi from 'joi';
import * as common from '@nestjs/common';
import type { ConfigService } from '@nestjs/config';
import { type Logger } from '@nestjs/common';
import legacy = require('legacy');
const local = 1;
export const all = [Get, Create, Joi, common, legacy, local, Proxy];
export let config: ConfigService | Logger;";
    with_file(text, |file| {
        let names: Vec<Option<(&str, &str)>> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::IdentifierReference(id) => Some(imported_name(file, id)),
                _ => None,
            })
            .collect();
        assert_eq!(
            names,
            [
                Some(("@nestjs/common", "Get")),
                Some(("@nestjs/common", "Post")),
                Some(("joi", "default")),
                None,
                None,
                None,
                None,
                Some(("@nestjs/config", "ConfigService")),
                Some(("@nestjs/common", "Logger")),
            ]
        );
    });
}

#[test]
fn imported_from_is_the_name_a_package_or_its_subpath_exports() {
    let text = "import { Get as Route } from '@nestjs/common';
import { Scope } from '@nestjs/common/interfaces';
import { Inject } from '@nestjs/commonx';
import { Get } from './common';
import * as common from '@nestjs/common';
export const all = [Route, Scope, Inject, Get, common];";
    with_file(text, |file| {
        let names: Vec<Option<&str>> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::IdentifierReference(id) => Some(imported_from(file, id, "@nestjs/common")),
                _ => None,
            })
            .collect();
        assert_eq!(names, [Some("Get"), Some("Scope"), None, None, None]);
    });
}

/// Each block as `[doc ]Kind [lines] + Kind [lines]`.
fn comment_parts_of(text: &str) -> Vec<String> {
    let mut blocks = Vec::new();
    with_file(text, |file| {
        for block in comment_blocks(file) {
            let parts: Vec<String> = block
                .parts
                .iter()
                .map(|part| {
                    let lines: Vec<&str> = part
                        .lines
                        .iter()
                        .map(|line| line.source_text(file.text))
                        .collect();
                    format!("{:?} {lines:?}", part.kind)
                })
                .collect();
            let doc = if block.doc { "doc " } else { "" };
            blocks.push(format!("{doc}{}", parts.join(" + ")));
        }
    });
    blocks
}

const COMMENT_CASES: &[(&str, &[&str])] = &[
    ("// One.\n// Two.\nx();", &[r#"Prose [" One.", " Two."]"#]),
    (
        "// One.\n\n// Two.\nx();",
        &[r#"Prose [" One."] + Prose [" Two."]"#],
    ),
    (
        "// The fallback:\n// await run();\nx();",
        &[r#"Prose [" The fallback:"] + Code [" await run();"]"#],
    ),
    (
        "// const a = 1;\n\n// return a;\nx();",
        &[r#"Code [" const a = 1;", " return a;"]"#],
    ),
    (
        "x(); // y();\n// z();\nw();",
        &[r#"Code [" y();"]"#, r#"Code [" z();"]"#],
    ),
    (
        "// eslint-disable-next-line no-console\n// The note.\nx();",
        &[
            r#"Directive [" eslint-disable-next-line no-console"]"#,
            r#"Prose [" The note."]"#,
        ],
    ),
    (
        "// @ts-nocheck\n// biome-ignore-all lint: generated\n// prettier-ignore\nx();",
        &[
            r#"Directive [" @ts-nocheck"]"#,
            r#"Directive [" biome-ignore-all lint: generated"]"#,
            r#"Directive [" prettier-ignore"]"#,
        ],
    ),
    (
        "/* global window */\n// global state is shared.\nx();",
        &[
            r#"Directive [" global window"]"#,
            r#"Prose [" global state is shared."]"#,
        ],
    ),
    (
        "/**\n * Adds.\n *\n *   add(1, 2);\n */\nx();",
        &[r#"doc Prose [" Adds."] + Code ["   add(1, 2);"]"#],
    ),
    (
        "// ```ts\n// add(1, 2);\n// ```\nx();",
        &[r#"Prose [" ```ts", " add(1, 2);", " ```"]"#],
    ),
    (
        "// Retries (3)\n// overwrites `x`\n// total = price * count\n// public API\n// Note: run()\n// Note: important,\n// - run()\nx();",
        &[
            r#"Prose [" Retries (3)", " overwrites `x`", " total = price * count", " public API", " Note: run()", " Note: important,", " - run()"]"#,
        ],
    ),
    (
        "class A {\n  // private readonly cache = new Map();\n  b = 1;\n  // @Get(':id')\n  // find() {}\n}\nconst o = {\n  // retries: 3,\n  // cause: pgCause,\n  a: 1,\n};",
        &[
            r#"Code [" private readonly cache = new Map();"]"#,
            r#"Code [" @Get(':id')", " find() {}"]"#,
            r#"Code [" retries: 3,", " cause: pgCause,"]"#,
        ],
    ),
    (
        "class A {\n  // @UseGuards(AuthGuard)\n  run() {}\n}",
        &[r#"Code [" @UseGuards(AuthGuard)"]"#],
    ),
    (
        "// await run();\n// done\nx();",
        &[r#"Code [" await run();"] + Prose [" done"]"#],
    ),
    ("f(/* force */ true);", &[r#"Prose [" force"]"#]),
    (
        "// Options\n// retries: 3,\n// cause: pgCause,\n// see the docs\nx();",
        &[
            r#"Prose [" Options"] + Code [" retries: 3,", " cause: pgCause,"] + Prose [" see the docs"]"#,
        ],
    ),
    (
        "// delete it\n// delete cache.key\nx();",
        &[r#"Prose [" delete it"] + Code [" delete cache.key"]"#],
    ),
    (
        "// const timeout = 30; // In seconds.\nx();",
        &[r#"Code [" const timeout = 30; // In seconds."]"#],
    ),
    (
        "// e.g. refresh()\n// config?.retries (3)\nx();",
        &[r#"Prose [" e.g. refresh()", " config?.retries (3)"]"#],
    ),
    (
        "// We wait for\n// init()\n// before reading the config.\nx();",
        &[r#"Prose [" We wait for", " init()", " before reading the config."]"#],
    ),
    (
        "// simple\n// const a = 1;\n// no leap years\nx();",
        &[r#"Prose [" simple"] + Code [" const a = 1;"] + Prose [" no leap years"]"#],
    ),
    (
        "/**\n * my-lib v1.2.3\n * @license MIT\n */\nx();",
        &[r#"Directive [" my-lib v1.2.3", " @license MIT"]"#],
    ),
    (
        "// Copyright 2024 Acme Inc\n//\n// Licensed under the MIT License\n// no default\n// NOSONAR\nx();",
        &[
            r#"Directive [" Copyright 2024 Acme Inc", " Licensed under the MIT License"]"#,
            r#"Directive [" no default"]"#,
            r#"Directive [" NOSONAR"]"#,
        ],
    ),
    ("/// Reads it.\nx();", &[r#"Prose [" Reads it."]"#]),
    (
        "// Кэш живёт минуту.\n// cache.clear();\nx();",
        &[r#"Prose [" Кэш живёт минуту."] + Code [" cache.clear();"]"#],
    ),
];

#[test]
fn comment_blocks_cut_comments_into_prose_code_and_directives() {
    for line_ending in ["\n", "\r\n"] {
        for &(case, expected) in COMMENT_CASES {
            let text = case.replace('\n', line_ending);
            assert_eq!(comment_parts_of(&text), expected, "{case:?}");
        }
    }
    with_file(
        "//   const a = 1;\n// The note\n// goes on.\nx();",
        |file| {
            let blocks = comment_blocks(file);
            let parts: Vec<&str> = blocks
                .iter()
                .flat_map(|block| &block.parts)
                .map(|part| part.span.source_text(file.text))
                .collect();
            assert_eq!(parts, ["const a = 1;", "The note\n// goes on."]);
            assert_eq!(
                blocks[0].span.source_text(file.text),
                "//   const a = 1;\n// The note\n// goes on."
            );
        },
    );
    // Each name ends the run it starts, so the code under a long list is still in reach.
    let names = "// abc\n".repeat(100);
    with_file(&format!("{names}// const a = 1;\nx();"), |file| {
        let parts = &comment_blocks(file)[0].parts;
        assert_eq!(parts.last().map(|part| part.kind), Some(CommentKind::Code));
    });
    with_file_at("src/a.tsx", "// <Header title=\"x\" />\nx();", |file| {
        assert_eq!(comment_blocks(file)[0].parts[0].kind, CommentKind::Code);
    });
}

#[test]
fn line_helpers_find_line_starts_blank_lines_regions_and_line_tails() {
    assert_eq!(line_start("ab\ncd", 4), 3);
    assert_eq!(line_start("ab", 1), 0);
    for (text, blank) in [
        ("\n", false),
        ("\n\n", true),
        ("\r\n\r\n", true),
        (" // note\n", false),
        ("x\n  \ny", true),
    ] {
        assert_eq!(has_blank_line(text), blank, "{text:?}");
    }
    for (comment, region) in [
        ("/* eslint-disable no-console */", true),
        ("/* eslint-enable */", true),
        ("// eslint-disable-next-line no-console", false),
        ("// eslint-disable-line", false),
        ("// oxlint-disable no-console", true),
        ("// oxlint-disable-next-line no-console", false),
        ("/* tslint:disable:max-line-length */", true),
        ("// tslint:disable-next-line", false),
        ("// tslint:enable-line", false),
        ("// biome-ignore-start lint: legacy", true),
        ("// biome-ignore-end lint: legacy", true),
        ("// biome-ignore-all lint: generated", true),
        ("// biome-ignore lint: legacy", false),
        ("// #region output", true),
        ("/* stylelint-disable */", true),
        ("// jscpd:ignore-start", true),
        ("// prettier-ignore", false),
        ("// a note", false),
    ] {
        assert_eq!(opens_region(comment), region, "{comment}");
    }
    for (text, tail) in [
        ("a(); // note\nb();", Some((12, true))),
        ("a() /* x */\nb();", Some((11, false))),
        ("a(); b();", None),
        ("a(); // biome-ignore lint/style: x\nb();", None),
        ("a(); // @ts-expect-error\nb();", None),
        ("a(); // prettier-ignore\nb();", None),
        ("a(); // language=SQL\nb();", None),
        ("a(); // cspell:disable-next-line\nb();", None),
        ("a(); // @ts-nocheck\nb();", Some((19, true))),
    ] {
        with_file(text, |file| {
            assert_eq!(line_tail(file, 3), tail, "{text}");
        });
    }
}

#[test]
fn one_line_joins_the_text_with_single_spaces() {
    assert_eq!(
        one_line("userIds\n      .filter(isActive)"),
        "userIds .filter(isActive)"
    );
    assert_eq!(
        one_line("{ REDIS_HOST,\r\n  REDIS_PORT }"),
        "{ REDIS_HOST, REDIS_PORT }"
    );
    assert_eq!(one_line("a  +\tb"), "a + b");
}

#[test]
fn static_text_is_a_string_or_a_template_without_substitutions() {
    with_file("f('a', `b`, `c${d}`, d, 'e' + d, 1);", |file| {
        let texts: Vec<Option<&str>> = file
            .semantic
            .nodes()
            .iter()
            .find_map(|node| match node.kind() {
                AstKind::CallExpression(call) => Some(
                    call.arguments
                        .iter()
                        .map(|argument| argument.as_expression().and_then(static_text))
                        .collect(),
                ),
                _ => None,
            })
            .unwrap();
        assert_eq!(texts, [Some("a"), Some("b"), None, None, None, None]);
    });
}

#[test]
fn literal_value_is_a_fixed_scalar_also_under_as_const() {
    let text = "const a = 'x', b = 5, c = -1, d = 60 * 1000, e = true, f = `t`, g = 'm' as const, \
                h = 1n, i = null, j = /x/, k = [1], l = { a: 1 }, m = 'n' as string, n = `${b}`, \
                o = !e, p = a + 'y';";
    with_file(text, |file| {
        let literals: Vec<bool> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::VariableDeclarator(declarator) => {
                    declarator.init.as_ref().map(is_literal_value)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            literals,
            [
                true, true, true, true, true, true, true, true, false, false, false, false, false,
                false, false, false
            ]
        );
    });
}

#[test]
fn literal_type_narrows_a_const_and_uses_that_compare_or_pass_it_notice() {
    let text = "declare function pick<T>(value: T): T;
declare function now(): number;
declare function tag(strings: TemplateStringsArray, ...values: unknown[]): string;
const a = 'x', b = null, c = a, d = a ? 'y' : 'z', e = pick('k'), f = tag`${a}`, g = now(),
  h = pick(now()), i = { a }, j = [a], k = () => a, l = new Date(), m = a + a, n = `${a}`, o = a as string,
  p = /x/, q = `t`, u = tag`plain`, v = this, w = pick(this);
export const shared = 1;
const kept = 'k', compared = 'c', passed = 'p', aliased = 's';
const uses = [`${kept}`, kept.length, !kept, kept + 1, compared === 'd', pick(passed)];
const alias = aliased;
let widened = aliased;
async function wait(): Promise<void> {
  const r = await pick('w'), s = await now();
  if (kept) {}
}
export { b, c, d, e, f, g, h, i, j, k, l, m, n, o, p, q, u, v, w, uses, alias, widened, wait };";
    with_file(text, |file| {
        let narrows: Vec<bool> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::VariableDeclarator(declarator) => {
                    declarator.init.as_ref().map(narrows_to_literal)
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            narrows,
            [
                true, true, true, true, true, true, false, false, false, false, false, false,
                false, false, false, false, true, false, false, false, true, true, true, true,
                true, false, true, true, true, false
            ]
        );
        let relies = |name: &str| relies_on_wide_type(file, symbol_named(file, name));
        assert!(!relies("kept"));
        assert!(relies("compared"));
        assert!(relies("passed"));
        // `const alias = aliased` keeps the literal type; `let` widens it again.
        assert!(relies("aliased"));
        assert!(relies("shared"));
    });
}

#[test]
fn known_array_is_an_array_by_its_own_syntax() {
    let text = "declare function load(): number[];
declare const rows: { filter(): number[] };
export function arrays(typed: number[], frozen: readonly number[], lib: Array<number>,
  read: ReadonlyArray<number>, pair: [number, number], maybe: number[] | undefined,
  untyped: unknown, set: Set<number>, ...more: number[]) {
  const literal = [1];
  const alias = literal;
  let written = [1];
  written = [2];
  let unwritten = [1];
  let grown: number[] = [];
  grown = [3];
  const [head]: [string, number[]] = ['a', [1]];
  const loaded = load();
  return [
    [1, 2], literal, alias, typed, frozen, lib, read, pair, maybe, typed.filter(Boolean),
    typed?.slice(), typed.map(String).sort(), Array.from(set), Array.of(1), Object.keys({}),
    Object.entries({}), literal as number[], literal!, grown, more, typed.with(0, 1),
    written, unwritten, head, loaded, untyped, set, rows.filter(), load(), typed.find(Boolean),
    typed.at(0),
  ];
}
export function shadowed(Array: { from(value: unknown): number[] }) {
  const first = second;
  const second = first;
  return [Array.from([]), first];
}";
    with_file(text, |file| {
        let known: Vec<bool> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::ReturnStatement(statement) => match &statement.argument {
                    Some(Expression::ArrayExpression(array)) => Some(array),
                    _ => None,
                },
                _ => None,
            })
            .flat_map(|array| array.elements.iter().filter_map(|e| e.as_expression()))
            .map(|element| is_known_array(file, element))
            .collect();
        assert_eq!(
            known,
            [
                true, true, true, true, true, true, true, true, true, true, true, true, true, true,
                true, true, true, true, true, true, true, false, false, false, false, false, false,
                false, false, false, false, false, false
            ]
        );
    });
}

#[test]
fn cli_script_starts_with_a_hashbang_or_sits_in_a_script_folder() {
    let dirs = ["scripts".to_string(), "bin".to_string()];
    for (path, text, is_script) in [
        ("src/seed.ts", "#!/usr/bin/env node\nexport {};", true),
        ("packages/db/scripts/seed.ts", "export {};", true),
        ("bin/cli.ts", "export {};", true),
        ("src/scripts-legacy/seed.ts", "export {};", false),
        ("src/scripts.ts", "export {};", false),
    ] {
        with_file_at(path, text, |file| {
            assert_eq!(is_cli_script(file, &dirs), is_script, "{path}");
        });
    }
}

#[test]
fn file_name_has_a_word_of_the_list_in_one_of_its_dotted_parts() {
    let list = ["env".to_string(), "Response".to_string()];
    for (path, has) in [
        ("src/require-env.ts", true),
        ("src/ENV.ts", true),
        ("src/order.response.dto.ts", true),
        ("src/orderResponse.ts", true),
        ("src/environment.ts", false),
        ("src/responses.ts", false),
        ("src/env/client.ts", false),
    ] {
        with_file_at(path, "export {};", |file| {
            assert_eq!(file_name_has_word(file, &list), has, "{path}");
        });
    }
}

#[test]
fn boolean_parameter_has_a_boolean_type_or_a_boolean_default() {
    let text = "export function f(a: boolean, b: true | null, c: boolean[], d = false, e = 0, f?: string) {
  return [a, b, c, d, e, f];
}";
    with_file(text, |file| {
        let booleans: Vec<bool> = file
            .semantic
            .nodes()
            .iter()
            .filter_map(|node| match node.kind() {
                AstKind::FormalParameter(parameter) => Some(is_boolean_parameter(parameter)),
                _ => None,
            })
            .collect();
        assert_eq!(booleans, [true, true, false, true, false, false]);
    });
}

/// The `probe()` calls, in source order.
fn probe_ids(file: &File<'_>) -> Vec<NodeId> {
    file.semantic
        .nodes()
        .iter()
        .filter(|node| {
            matches!(node.kind(), AstKind::CallExpression(call)
                if matches!(&call.callee, Expression::Identifier(id) if id.name == "probe"))
        })
        .map(AstNode::id)
        .collect()
}

fn probes<T>(file: &File<'_>, of: impl Fn(NodeId) -> T) -> Vec<T> {
    probe_ids(file).into_iter().map(of).collect()
}

fn loops_of_probes<'a>(file: &File<'a>) -> Vec<Option<(LoopKind, Option<&'a str>, u32)>> {
    probes(file, |probe| loop_around(file, probe))
        .into_iter()
        .map(|around| {
            around.map(|around| {
                let over = around.over.map(|over| source(file, over));
                (around.kind, over, around.depth)
            })
        })
        .collect()
}

#[test]
fn loop_around_names_each_kind_of_loop_its_collection_and_the_depth() {
    let text = "declare function probe(): void;
declare const rows: number[];
declare const stream: AsyncIterable<number> & { on(event: string, listener: () => void): void };
declare const index: Record<string, number>;
export async function f() {
  for (let i = 0; i < rows.length; i++) probe();
  for (const row of rows) probe();
  for await (const chunk of stream) probe();
  for (const key in index) probe();
  while (rows.length) probe();
  do probe(); while (rows.length);
  rows.forEach(() => probe());
  stream.on('data', () => probe());
  for (const row of rows) rows.map(() => rows.forEach(() => probe()));
}";
    with_file(text, |file| {
        assert_eq!(
            loops_of_probes(file),
            [
                Some((LoopKind::For, Some("rows"), 1)),
                Some((LoopKind::ForOf, Some("rows"), 1)),
                Some((LoopKind::ForAwaitOf, Some("stream"), 1)),
                Some((LoopKind::ForIn, Some("index"), 1)),
                Some((LoopKind::While, None, 1)),
                Some((LoopKind::DoWhile, None, 1)),
                Some((LoopKind::Callback, Some("rows"), 1)),
                Some((LoopKind::Listener, None, 1)),
                // Through the `map` callback to the `for..of` around it.
                Some((LoopKind::Callback, Some("rows"), 3)),
            ]
        );
        let labels: Vec<String> = probes(file, |probe| loop_around(file, probe))
            .into_iter()
            .map(|around| around.expect("a loop").label(file))
            .collect();
        assert_eq!(
            labels,
            [
                "`for` loop over `rows`",
                "`for..of` loop over `rows`",
                "`for await` loop over `stream`",
                "`for..in` loop over `index`",
                "`while` loop",
                "`do..while` loop",
                "`forEach` callback over `rows`",
                "`on` listener",
                "`forEach` callback over `rows`",
            ]
        );
    });
}

#[test]
fn loop_around_skips_what_runs_once_and_stops_at_other_functions() {
    let text = "declare function probe(): number;
declare function later(run: () => void): void;
declare const rows: number[];
export function g() {
  probe();
  for (let i = probe(); i < 3; i++) {}
  for (const row of [probe()]) {}
  for (let i = 0; probe() > i; i += probe()) {}
  for (const row of rows) {
    const run = () => probe();
    later(() => probe());
    rows.forEach(() => probe(), probe());
  }
  while (rows.length) {
    function inner() { probe(); }
  }
  for (let i = probe(); i < 3; i++) for (const row of rows) probe();
}";
    with_file(text, |file| {
        assert_eq!(
            loops_of_probes(file),
            [
                None,
                // The `init` of `for` and the collection of `for..of` run once.
                None,
                None,
                // The test and the update run on every pass.
                Some((LoopKind::For, None, 1)),
                Some((LoopKind::For, None, 1)),
                // A function that is not a per-item callback runs whenever it is called.
                None,
                None,
                Some((LoopKind::Callback, Some("rows"), 2)),
                // The second argument of `forEach` runs once per pass of the `for..of`.
                Some((LoopKind::ForOf, Some("rows"), 1)),
                None,
                None,
                Some((LoopKind::ForOf, Some("rows"), 2)),
            ]
        );
        let &[.., init, body] = probe_ids(file).as_slice() else {
            panic!("the probes of the last loops");
        };
        let inner = loop_around(file, body).expect("the inner loop");
        let outer = loop_around(file, inner.node.id()).expect("the outer loop");
        // The `init` of the outer `for` runs once; the body of the inner loop runs on every pass
        // of both.
        assert_eq!(
            [
                outer.runs_each_pass(file, init),
                outer.runs_each_pass(file, body),
                inner.runs_each_pass(file, body),
            ],
            [false, true, true]
        );
    });
}

#[test]
fn nest_role_names_the_role_of_the_method_around_a_node_in_a_class_nest_decorates() {
    let text = "import { Controller, Get, Injectable } from '@nestjs/common';
import { MessagePattern } from '@nestjs/microservices';
import { WebSocketGateway } from '@nestjs/websockets';
declare function probe(): void;
probe();
@Controller('orders')
export class OrdersController {
  constructor() { probe(); }
  @Get(':id')
  find() { probe(); }
  @MessagePattern('orders.created')
  created() { [1].forEach(() => probe()); }
  onModuleInit() { probe(); }
  private format() { probe(); }
  @Get(String(probe()))
  other() {}
}
@Injectable()
export class OrdersService {
  load() { probe(); }
  static create() { probe(); }
  afterInit() { probe(); }
  run() { class Inner { m() { probe(); } } }
}
@WebSocketGateway()
export class EventsGateway {
  afterInit() { probe(); }
}
export class Plain {
  load() { probe(); }
}";
    with_file(text, |file| {
        assert_eq!(
            probes(file, |probe| nest_role(file, probe)),
            [
                None,
                Some(NestRole::Constructor),
                Some(NestRole::RouteHandler),
                // Through the callback to the method around it.
                Some(NestRole::MessageHandler),
                Some(NestRole::Lifecycle),
                Some(NestRole::ProviderMethod),
                // A decorator's argument runs once, when the class is declared.
                None,
                Some(NestRole::ProviderMethod),
                Some(NestRole::ProviderMethod),
                // `afterInit` is a hook of a gateway only.
                Some(NestRole::ProviderMethod),
                // The innermost class decides.
                None,
                Some(NestRole::Lifecycle),
                None,
            ]
        );
    });
}
