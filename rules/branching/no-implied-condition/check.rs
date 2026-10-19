use kit::oxc::ast::ast::{
    BinaryExpression, BinaryOperator, Expression, IdentifierReference, IfStatement,
    LogicalOperator, Statement, UnaryOperator, VariableDeclarationKind,
};
use kit::oxc::semantic::SymbolId;
use kit::oxc::span::ContentEq;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::IdentifierReference, AstType::BinaryExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let Some(flipped) = decides(file, node) else {
            return;
        };
        let test = match node.kind() {
            AstKind::IdentifierReference(id) => Some(Test::truthy(id)),
            AstKind::BinaryExpression(binary) => compared(file, binary),
            _ => None,
        };
        let Some(test) = test else {
            return;
        };
        let Some((symbol, binding)) = stable(file, test.id) else {
            return;
        };
        let Some(proof) = proven(file, node, symbol, test.form, test.negated != flipped) else {
            return;
        };
        let value = match (test.form, proof.holds != test.negated) {
            (Form::Truthy, true) => "truthy",
            (Form::Truthy, false) => "falsy",
            (Form::Equals { .. }, true) => "true",
            (Form::Equals { .. }, false) => "false",
        };
        cx.report(node.span())
            .arg("condition", one_line(node.span().source_text(file.text)))
            .arg("value", value)
            .arg("check", proof.check)
            .arg("line", file.lines.position(proof.at.start).line)
            .arg("name", test.id.name.as_str())
            .arg("binding", binding);
    }
}

struct Test<'a> {
    id: &'a IdentifierReference<'a>,
    form: Form<'a>,
    /// `!==` and `!=`: the same test as `===` and `==`, negated.
    negated: bool,
}

#[derive(Clone, Copy)]
enum Form<'a> {
    /// `user`.
    Truthy,
    /// `status === 'paid'`, `typeof value === 'string'`, `id == null`.
    Equals {
        of_type: bool,
        strict: bool,
        literal: &'a Expression<'a>,
    },
}

impl<'a> Test<'a> {
    fn truthy(id: &'a IdentifierReference<'a>) -> Self {
        Test {
            id,
            form: Form::Truthy,
            negated: false,
        }
    }
}

impl Form<'_> {
    fn is(self, other: Form<'_>) -> bool {
        match (self, other) {
            (Form::Truthy, Form::Truthy) => true,
            (
                Form::Equals {
                    of_type,
                    strict,
                    literal,
                },
                Form::Equals {
                    of_type: other_type,
                    strict: other_strict,
                    literal: other_literal,
                },
            ) => {
                of_type == other_type && strict == other_strict && literal.content_eq(other_literal)
            }
            _ => false,
        }
    }
}

fn test_of<'a>(file: &File<'a>, expression: &'a Expression<'a>) -> Option<Test<'a>> {
    match expression {
        Expression::Identifier(id) => Some(Test::truthy(id)),
        Expression::BinaryExpression(binary) => compared(file, binary),
        _ => None,
    }
}

/// `x === 'a'`, `'a' !== x`, `typeof x === 'string'`; `==` and `!=` only with `null` or
/// `undefined`, which call no `valueOf`.
fn compared<'a>(file: &File<'a>, binary: &'a BinaryExpression<'a>) -> Option<Test<'a>> {
    let (strict, negated) = match binary.operator {
        BinaryOperator::StrictEquality => (true, false),
        BinaryOperator::StrictInequality => (true, true),
        BinaryOperator::Equality => (false, false),
        BinaryOperator::Inequality => (false, true),
        _ => return None,
    };
    let (left, right) = (
        binary.left.get_inner_expression(),
        binary.right.get_inner_expression(),
    );
    let (subject, literal) = if is_literal(file, right) {
        (left, right)
    } else if is_literal(file, left) {
        (right, left)
    } else {
        return None;
    };
    if !strict && !is_nullish(file, literal) {
        return None;
    }
    let (id, of_type) = match subject {
        Expression::Identifier(id) => (id, false),
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::Typeof => {
            let Expression::Identifier(id) = unary.argument.get_inner_expression() else {
                return None;
            };
            (id, true)
        }
        _ => return None,
    };
    let form = Form::Equals {
        of_type,
        strict,
        literal,
    };
    Some(Test { id, form, negated })
}

fn is_literal(file: &File<'_>, expression: &Expression<'_>) -> bool {
    is_literal_value(expression) || is_nullish(file, expression)
}

fn is_nullish(file: &File<'_>, expression: &Expression<'_>) -> bool {
    matches!(expression, Expression::NullLiteral(_)) || is_global(file, expression, "undefined")
}

fn symbol_of(file: &File<'_>, id: &IdentifierReference<'_>) -> Option<SymbolId> {
    file.semantic
        .scoping()
        .get_reference(id.reference_id())
        .symbol_id()
}

/// The symbol and how the message names its binding, when nothing can change what the name
/// holds.
fn stable(file: &File<'_>, id: &IdentifierReference<'_>) -> Option<(SymbolId, &'static str)> {
    let symbol = symbol_of(file, id)?;
    let nodes = file.semantic.nodes();
    let declaration = file.semantic.scoping().symbol_declaration(symbol);
    let binding = match nodes.kind(declaration) {
        AstKind::FormalParameter(_) | AstKind::FormalParameterRest(_) => {
            "a parameter nothing reassigns"
        }
        AstKind::VariableDeclarator(_) => match nodes.parent_kind(declaration) {
            AstKind::VariableDeclaration(list) if list.kind == VariableDeclarationKind::Const => {
                "a `const`"
            }
            _ => return None,
        },
        _ => return None,
    };
    (!file.semantic.scoping().symbol_is_mutated(symbol)).then_some((symbol, binding))
}

/// Whether the value of the node decides a branch: the test of `if`, `?:` or a loop, or the
/// left side of `&&` or `||`, also through `!` and the right side of such an operator. `Some`
/// says whether an odd number of `!` stands between. `??` tests for `null`, not for truth.
fn decides(file: &File<'_>, node: &AstNode<'_>) -> Option<bool> {
    let mut span = node.span();
    let mut flipped = false;
    for parent in file.semantic.nodes().ancestors(node.id()) {
        let child = std::mem::replace(&mut span, parent.span());
        let test = match parent.kind() {
            AstKind::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
                flipped = !flipped;
                continue;
            }
            AstKind::TSAsExpression(_)
            | AstKind::TSSatisfiesExpression(_)
            | AstKind::TSNonNullExpression(_)
            | AstKind::TSTypeAssertion(_) => continue,
            AstKind::LogicalExpression(logical)
                if logical.operator != LogicalOperator::Coalesce =>
            {
                if logical.right.span() == child {
                    continue;
                }
                &logical.left
            }
            AstKind::IfStatement(statement) => &statement.test,
            AstKind::ConditionalExpression(conditional) => &conditional.test,
            AstKind::WhileStatement(statement) => &statement.test,
            AstKind::DoWhileStatement(statement) => &statement.test,
            AstKind::ForStatement(statement) => statement.test.as_ref()?,
            _ => return None,
        };
        return (test.span() == child).then_some(flipped);
    }
    None
}

struct Proof {
    /// The value the test of the node, without its own `!==`, has here.
    holds: bool,
    /// "the `if`", "the `&&`".
    check: &'static str,
    /// The expression that proved it.
    at: Span,
}

/// What a test around the node already proved about the test on `symbol`, the innermost first.
/// `polarity` is the number of negations, odd or even, between that test and the decision it is
/// part of. The walk stops at a function and at a class field or static block: TypeScript before
/// 5.4 forgets a narrowed parameter inside a callback, and a function declaration or a class
/// member forgets even a narrowed `const`.
fn proven<'a>(
    file: &File<'a>,
    node: &AstNode<'a>,
    symbol: SymbolId,
    form: Form<'_>,
    polarity: bool,
) -> Option<Proof> {
    let mut child = node.span();
    for ancestor in file.semantic.nodes().ancestors(node.id()) {
        let source = match ancestor.kind() {
            AstKind::IfStatement(statement) if statement.test.span() != child => Some((
                &statement.test,
                statement.consequent.span() == child,
                "the `if`",
            )),
            AstKind::ConditionalExpression(conditional) if conditional.test.span() != child => {
                let value = conditional.consequent.span() == child;
                Some((&conditional.test, value, "the `?:`"))
            }
            AstKind::LogicalExpression(logical) if logical.right.span() == child => {
                match logical.operator {
                    LogicalOperator::And => Some((&logical.left, true, "the `&&`")),
                    LogicalOperator::Or => Some((&logical.left, false, "the `||`")),
                    LogicalOperator::Coalesce => None,
                }
            }
            AstKind::Function(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::PropertyDefinition(_)
            | AstKind::AccessorProperty(_)
            | AstKind::StaticBlock(_) => return None,
            _ => None,
        };
        if let Some((expression, value, check)) = source
            && let Some(holds) = implied(file, expression, value, symbol, form)
            // An `else if` test that comes out false repeats an earlier test of its chain, and
            // ESLint's `no-dupe-else-if` reports the whole test when it does. A part of one is
            // left unreported: `b` in `else if (a || b)` after `if (b)`.
            && !(holds == polarity
                && matches!(ancestor.kind(), AstKind::IfStatement(statement)
                    if in_chain_test(statement, node.span())))
        {
            return Some(Proof {
                holds,
                check,
                at: expression.span(),
            });
        }
        child = ancestor.span();
    }
    None
}

/// The node sits in the test of an `else if` of the chain that `statement` starts.
fn in_chain_test(statement: &IfStatement<'_>, node: Span) -> bool {
    let mut next = statement.alternate.as_ref();
    while let Some(Statement::IfStatement(inner)) = next {
        if inner.test.span().contains_inclusive(node) {
            return true;
        }
        next = inner.alternate.as_ref();
    }
    false
}

/// The value of the test on `symbol` (without its own negation) when `expression` has `value`:
/// `a && b` true proves `a` and `b`, `a || b` false proves both false, `!a` flips.
fn implied<'a>(
    file: &File<'a>,
    expression: &'a Expression<'a>,
    value: bool,
    symbol: SymbolId,
    form: Form<'_>,
) -> Option<bool> {
    match expression.get_inner_expression() {
        Expression::UnaryExpression(unary) if unary.operator == UnaryOperator::LogicalNot => {
            implied(file, &unary.argument, !value, symbol, form)
        }
        Expression::LogicalExpression(logical)
            if (logical.operator == LogicalOperator::And && value)
                || (logical.operator == LogicalOperator::Or && !value) =>
        {
            implied(file, &logical.left, value, symbol, form)
                .or_else(|| implied(file, &logical.right, value, symbol, form))
        }
        expression => {
            let test = test_of(file, expression)?;
            (symbol_of(file, test.id) == Some(symbol) && test.form.is(form))
                .then_some(value != test.negated)
        }
    }
}
