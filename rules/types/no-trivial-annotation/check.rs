use kit::oxc::ast::ast::{
    BindingPattern, Expression, TSType, UnaryOperator, VariableDeclarationKind,
};
use kit::oxc::semantic::SymbolId;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::VariableDeclarator];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::VariableDeclarator(declarator) = node.kind() else {
            return;
        };
        let (BindingPattern::BindingIdentifier(id), Some(annotation), Some(init)) = (
            &declarator.id,
            &declarator.type_annotation,
            &declarator.init,
        ) else {
            return;
        };
        let ty = &annotation.type_annotation;
        let Some(literal) = repeated_type(file, ty, init) else {
            return;
        };
        let constant = matches!(
            file.semantic.nodes().parent_kind(node.id()),
            AstKind::VariableDeclaration(declaration)
                if declaration.kind == VariableDeclarationKind::Const
        );
        // Without the annotation a `const` holding a literal gets the literal type (`5`, not
        // `number`).
        let narrowing_noticed = literal
            && constant
            && id
                .symbol_id
                .get()
                .is_some_and(|symbol| relies_on_wide_type(file, symbol));
        let reported = cx
            .report(Span::new(id.span.start, annotation.span.end))
            .arg("type", ty.span().source_text(file.text));
        if !narrowing_noticed {
            reported.fix([Edit {
                span: annotation.span,
                text: String::new(),
            }]);
        }
    }
}

/// `Some` when the value gives the annotated type by itself; `Some(true)` when the value is a
/// literal that a `const` would keep as its own type.
fn repeated_type(file: &File<'_>, ty: &TSType<'_>, init: &Expression<'_>) -> Option<bool> {
    let unsigned = match init {
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::UnaryNegation => {
            &unary.argument
        }
        _ => init,
    };
    let primitive = match ty {
        TSType::TSNumberKeyword(_) => matches!(unsigned, Expression::NumericLiteral(_)),
        TSType::TSBigIntKeyword(_) => matches!(unsigned, Expression::BigIntLiteral(_)),
        TSType::TSStringKeyword(_) => matches!(
            init,
            Expression::StringLiteral(_) | Expression::TemplateLiteral(_)
        ),
        TSType::TSBooleanKeyword(_) => matches!(init, Expression::BooleanLiteral(_)),
        _ => false,
    };
    if primitive {
        // `hello ${name}` is a `string` even in a `const`.
        return Some(
            !matches!(init, Expression::TemplateLiteral(template) if !template.expressions.is_empty()),
        );
    }
    let same = match init {
        Expression::RegExpLiteral(_) => {
            lib_type(file, ty, "RegExp").is_some_and(|regexp| regexp.type_arguments.is_none())
        }
        // `Map<string, Order> = new Map()` differs: without the annotation it is `Map<any, any>`.
        Expression::NewExpression(new) if matches!(ty, TSType::TSTypeReference(_)) => {
            let class_end = new
                .type_arguments
                .as_ref()
                .map_or(new.callee.span().end, |arguments| arguments.span.end);
            let class = Span::new(new.callee.span().start, class_end);
            same_without_spaces(
                ty.span().source_text(file.text),
                class.source_text(file.text),
            )
        }
        _ => false,
    };
    same.then_some(false)
}

fn same_without_spaces(a: &str, b: &str) -> bool {
    let solid = |c: &char| !c.is_whitespace();
    a.chars().filter(solid).eq(b.chars().filter(solid))
}

/// Another file, or any use but one that treats a literal like its primitive, may rely on the
/// wide type and stop compiling with the literal one: `status === 'closed'`, a generic call
/// `pick(status)`, an alias `const current = status`.
fn relies_on_wide_type(file: &File<'_>, symbol: SymbolId) -> bool {
    let nodes = file.semantic.nodes();
    is_exported(file, symbol)
        || file
            .semantic
            .scoping()
            .get_resolved_references(symbol)
            .any(|reference| !keeps_primitive(file, nodes.get_node(reference.node_id())))
}

/// A use where `'open'` behaves like `string` and `5` like `number`: `${status}`,
/// `count + 1`, `count < limit`, `status.length`, `!flag`, the test of `if`, a loop or `?:`,
/// the value of a `let`, `var` or annotated variable, which widens it again.
fn keeps_primitive(file: &File<'_>, node: &AstNode<'_>) -> bool {
    let nodes = file.semantic.nodes();
    let span = node.span();
    let parent = nodes.parent_node(node.id());
    match parent.kind() {
        // A tag function may be generic over its values.
        AstKind::TemplateLiteral(_) => !matches!(
            nodes.parent_kind(parent.id()),
            AstKind::TaggedTemplateExpression(_)
        ),
        AstKind::BinaryExpression(binary) => {
            binary.operator.is_numeric_or_string_binary_operator() || binary.operator.is_compare()
        }
        AstKind::UnaryExpression(unary) => unary.operator != UnaryOperator::Delete,
        AstKind::StaticMemberExpression(member) => member.object.span() == span,
        AstKind::ComputedMemberExpression(member) => member.object.span() == span,
        AstKind::PrivateFieldExpression(member) => member.object.span() == span,
        AstKind::IfStatement(statement) => statement.test.span() == span,
        AstKind::WhileStatement(statement) => statement.test.span() == span,
        AstKind::DoWhileStatement(statement) => statement.test.span() == span,
        AstKind::ForStatement(statement) => statement
            .test
            .as_ref()
            .is_some_and(|test| test.span() == span),
        AstKind::ConditionalExpression(conditional) => conditional.test.span() == span,
        AstKind::ExpressionStatement(_) => true,
        AstKind::VariableDeclarator(declarator) => {
            declarator.type_annotation.is_some()
                || matches!(
                    nodes.parent_kind(parent.id()),
                    AstKind::VariableDeclaration(declaration)
                        if matches!(
                            declaration.kind,
                            VariableDeclarationKind::Let | VariableDeclarationKind::Var
                        )
                )
        }
        _ => false,
    }
}
