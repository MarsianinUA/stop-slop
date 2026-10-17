use kit::Reported;
use kit::oxc::ast::ast::{
    BinaryOperator, BindingPattern, Expression, VariableDeclarationKind, VariableDeclarator,
};
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

// The first case of a list is the one a fix renames to.
const PASCAL: &[Case] = &[Case::Pascal];
const CAMEL: &[Case] = &[Case::Camel];
const CONSTANT: &[Case] = &[Case::Constant];
const CAMEL_OR_CONSTANT: &[Case] = &[Case::Camel, Case::Constant];
/// A function may be a decorator factory (`Roles()`) or a component; a parameter, a call result
/// or a value built with `new` may be a class (`build(Target)`, `const MockLogger = jest.fn()`).
const CAMEL_OR_PASCAL: &[Case] = &[Case::Camel, Case::Pascal];
/// The key decides a destructured name (a class, an enum member, a constant), and a module
/// `const` may be an instance, a schema, a DI token or an enum-like object.
const ANY: &[Case] = &[Case::Camel, Case::Pascal, Case::Constant];

impl Rule for Check {
    const NODES: &'static [AstType] = PRIVATE_MEMBER_NODES;

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some((name, span)) = private_member(file, node) else {
            return;
        };
        let cases: &[Case] = match node.kind() {
            // The field `no-constant-in-class-file` moves a module constant to.
            AstKind::PropertyDefinition(property) if property.r#static && property.readonly => {
                CAMEL_OR_CONSTANT
            }
            _ => CAMEL,
        };
        if let Some(suggestion) = suggestion(name, cases) {
            // With its `#`, as the reader sees it.
            let shown = span.source_text(file.text);
            report(
                cx,
                span,
                shown,
                cases,
                &shown.replacen(name, &suggestion, 1),
            );
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        for symbol in scoping.symbol_ids() {
            let Some(cases) = expected(file, symbol) else {
                continue;
            };
            let name = scoping.symbol_name(symbol);
            let Some(suggestion) = suggestion(name, cases) else {
                continue;
            };
            if !is_own_name(file, symbol) {
                continue;
            }
            report(cx, scoping.symbol_span(symbol), name, cases, &suggestion)
                .fix(rename(file, symbol, &suggestion).unwrap_or_default());
        }
    }
}

fn report<'c>(
    cx: &'c mut Cx<'_, '_>,
    span: Span,
    name: &str,
    cases: &[Case],
    suggestion: &str,
) -> Reported<'c> {
    let case = cases
        .iter()
        .map(|case| match case {
            Case::Pascal => "PascalCase",
            Case::Camel => "camelCase",
            Case::Constant => "CONSTANT_CASE",
            Case::Kebab => "kebab-case",
        })
        .collect::<Vec<_>>()
        .join(" or ");
    cx.report(span)
        .arg("name", name)
        .arg("case", case)
        .arg("suggestion", suggestion)
}

/// `name` in the first of `cases`, or `None` when it fits one already or no case can change it
/// (`_2fa`, a script without capitals). Leading `_` are `no-underscore-prefix`'s: the case is
/// checked without them, and the suggestion keeps them.
fn suggestion(name: &str, cases: &[Case]) -> Option<String> {
    let bare = name.trim_start_matches('_');
    if cases.iter().any(|case| case.matches(bare)) {
        return None;
    }
    let prefix = &name[..name.len() - bare.len()];
    cases[0]
        .apply(bare)
        .map(|bare| format!("{prefix}{bare}"))
        .filter(|suggestion| suggestion != name)
}

/// The cases the symbol's declaration allows, PascalCase for the symbol of a JSX tag; `None` for
/// a catch parameter, which is `catch-param-name`'s.
fn expected(file: &File<'_>, symbol: SymbolId) -> Option<&'static [Case]> {
    let declaration = file.semantic.scoping().symbol_declaration(symbol);
    let cases = match file.semantic.nodes().kind(declaration) {
        AstKind::Class(_)
        | AstKind::TSInterfaceDeclaration(_)
        | AstKind::TSTypeAliasDeclaration(_)
        | AstKind::TSEnumDeclaration(_)
        | AstKind::TSTypeParameter(_)
        | AstKind::TSMappedType(_) => PASCAL,
        AstKind::Function(_) => CAMEL_OR_PASCAL,
        AstKind::FormalParameter(parameter) => parameter_cases(&parameter.pattern),
        AstKind::FormalParameterRest(parameter) => parameter_cases(&parameter.rest.argument),
        AstKind::VariableDeclarator(declarator) => variable_cases(file, symbol, declarator),
        _ => return None,
    };
    // `<icon />` is an HTML element, so the symbol of a JSX tag needs the capital.
    Some(if is_jsx_tag(file, symbol) {
        PASCAL
    } else if cases == CAMEL && is_used_as_class(file, symbol) {
        CAMEL_OR_PASCAL
    } else {
        cases
    })
}

/// `new Model()`, `extends Model`, `x instanceof Model`: the value is a class, as in
/// `const Model = isAdmin ? Admin : Guest`.
fn is_used_as_class(file: &File<'_>, symbol: SymbolId) -> bool {
    let nodes = file.semantic.nodes();
    file.semantic
        .scoping()
        .get_resolved_references(symbol)
        .any(|reference| {
            let span = nodes.get_node(reference.node_id()).span();
            match nodes.parent_kind(reference.node_id()) {
                AstKind::NewExpression(new) => new.callee.span() == span,
                AstKind::Class(class) => class
                    .heritage
                    .as_ref()
                    .is_some_and(|base| base.expression.span() == span),
                AstKind::BinaryExpression(binary) => {
                    binary.operator == BinaryOperator::Instanceof && binary.right.span() == span
                }
                _ => false,
            }
        })
}

fn parameter_cases(pattern: &BindingPattern<'_>) -> &'static [Case] {
    if matches!(pattern, BindingPattern::BindingIdentifier(_)) {
        CAMEL_OR_PASCAL
    } else {
        ANY
    }
}

fn variable_cases(
    file: &File<'_>,
    symbol: SymbolId,
    declarator: &VariableDeclarator<'_>,
) -> &'static [Case] {
    if !matches!(declarator.id, BindingPattern::BindingIdentifier(_)) {
        return ANY;
    }
    let scoping = file.semantic.scoping();
    let is_const = matches!(
        file.semantic.nodes().parent_kind(declarator.node_id.get()),
        AstKind::VariableDeclaration(declaration) if declaration.kind == VariableDeclarationKind::Const
    );
    let in_module = scoping.symbol_scope_id(symbol) == scoping.root_scope_id();
    // The value with its assertion: `'a' as string` is not a fixed value.
    let value = declarator.init.as_ref();
    match value.map(Expression::get_inner_expression) {
        Some(Expression::FunctionExpression(_) | Expression::ArrowFunctionExpression(_)) => {
            CAMEL_OR_PASCAL
        }
        Some(Expression::ClassExpression(_)) => PASCAL,
        _ if !is_const => CAMEL,
        _ if in_module && value.is_some_and(is_literal_value) => CONSTANT,
        _ if in_module => ANY,
        Some(Expression::CallExpression(_) | Expression::AwaitExpression(_)) => CAMEL_OR_PASCAL,
        _ => CAMEL,
    }
}
