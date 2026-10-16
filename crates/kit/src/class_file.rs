use oxc_ast::ast::{
    ArrowFunctionExpression, Class, Declaration, Decorator, ExportDefaultDeclarationKind,
    Expression, Function, Statement, VariableDeclaration,
};
use oxc_semantic::SymbolId;
use oxc_span::{GetSpan, Span};

use crate::File;

pub struct TopClass<'a> {
    /// The class's own name; else the variable a class expression initializes
    /// (`const A = class {}`); `default` for `export default class {}`.
    pub name: &'a str,
    /// `class A` / `abstract class A` without decorators when the class has a name; the
    /// variable's name for a class expression without one; `class` for an anonymous default export.
    pub head: Span,
}

/// The classes declared at the top of the file, in order: `class A {}`, under `export` or
/// `export default` or not, and a class expression that is the whole value of a top-level
/// `const` (through parentheses and `as`); a `let` / `var` holding one is only a variable binding.
/// Not `declare class`, and not a class inside a function, a namespace or another expression.
/// A file with one is a file with a class.
pub fn top_classes<'a>(file: &File<'a>) -> Vec<TopClass<'a>> {
    let mut classes = Vec::new();
    for top in top_declarations(file) {
        match top {
            Top::Class(class) => classes.push(TopClass {
                name: class.id.as_ref().map_or("default", |id| id.name.as_str()),
                head: class_head(file, class),
            }),
            Top::Variables(variables) if variables.kind.is_const() => {
                for declarator in &variables.declarations {
                    let Some(Expression::ClassExpression(class)) = declarator
                        .init
                        .as_ref()
                        .map(Expression::get_inner_expression)
                    else {
                        continue;
                    };
                    let Some(variable) = declarator.id.get_binding_identifier() else {
                        continue;
                    };
                    let (name, head) = match &class.id {
                        Some(id) => (id.name.as_str(), class_head(file, class)),
                        None => (variable.name.as_str(), variable.span),
                    };
                    classes.push(TopClass { name, head });
                }
            }
            _ => {}
        }
    }
    classes
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopKind {
    /// `function f() {}` with a body (not an overload signature, not `declare`),
    /// `export default function`, `export default () => ..`, and a `const` whose value is a
    /// function or an arrow.
    Function,
    /// Any other `const`; a `const` whose value is a class expression is a class, not a binding.
    Constant,
    /// A `let` or `var`, whatever it holds.
    Variable,
}

/// A function or a `const` / `let` / `var` binding at the top of the file.
pub struct TopBinding<'a> {
    pub kind: TopKind,
    /// The name or the destructuring pattern (`{ a, b }`); `function (..)` or `(..)` of an
    /// anonymous default export.
    pub span: Span,
    /// The bound symbols: one, several for a pattern, none for an anonymous default export
    /// (the name of `export default (function f() {})` is bound only inside it).
    pub symbols: Vec<SymbolId>,
    /// The value of a `const` / `let` / `var`; `None` for a function declaration, an anonymous
    /// default export (`export default () => ..`) and a declarator without one.
    pub init: Option<&'a Expression<'a>>,
}

/// Functions and bindings declared at the top of the file, in order, under `export` /
/// `export default` or not. Not under `declare`, not inside a namespace or a block, not
/// `using`; an overloaded function is one binding.
pub fn top_bindings<'a>(file: &File<'a>) -> Vec<TopBinding<'a>> {
    let mut bindings = Vec::new();
    for top in top_declarations(file) {
        match top {
            Top::Function(function) if function.body.is_some() => {
                bindings.push(TopBinding {
                    kind: TopKind::Function,
                    span: function.id.as_ref().map_or(
                        Span::new(function.span.start, function.params.span.end),
                        |id| id.span,
                    ),
                    symbols: function
                        .id
                        .iter()
                        .filter(|_| function.is_declaration())
                        .filter_map(|id| id.symbol_id.get())
                        .collect(),
                    init: None,
                });
            }
            Top::Arrow(arrow) => bindings.push(TopBinding {
                kind: TopKind::Function,
                span: arrow.params.span,
                symbols: Vec::new(),
                init: None,
            }),
            Top::Variables(variables) if !variables.kind.is_using() => {
                for declarator in &variables.declarations {
                    let init = declarator.init.as_ref();
                    let kind = if variables.kind.is_const() {
                        match init.map(Expression::get_inner_expression) {
                            Some(Expression::ClassExpression(_)) => continue,
                            Some(value) if value.is_function() => TopKind::Function,
                            _ => TopKind::Constant,
                        }
                    } else {
                        TopKind::Variable
                    };
                    bindings.push(TopBinding {
                        kind,
                        span: declarator.id.span(),
                        symbols: declarator
                            .id
                            .get_binding_identifiers()
                            .iter()
                            .filter_map(|id| id.symbol_id.get())
                            .collect(),
                        init,
                    });
                }
            }
            _ => {}
        }
    }
    bindings
}

/// A declaration at the top of the file, with `export` / `export default` taken off and
/// `declare` left out; the value of `export default` goes through parentheses and `as`.
enum Top<'a> {
    Class(&'a Class<'a>),
    Function(&'a Function<'a>),
    Arrow(&'a ArrowFunctionExpression<'a>),
    Variables(&'a VariableDeclaration<'a>),
}

fn top_declarations<'a>(file: &File<'a>) -> impl Iterator<Item = Top<'a>> {
    let body = &file.semantic.nodes().program().body;
    body.iter().filter_map(|statement| {
        let declaration = match statement {
            Statement::ExportDeclaration(export) => &export.declaration,
            Statement::ExportDefaultDeclaration(export) => return default_top(&export.declaration),
            statement => statement.as_declaration()?,
        };
        match declaration {
            Declaration::ClassDeclaration(class) if !class.declare => Some(Top::Class(class)),
            Declaration::FunctionDeclaration(function) if !function.declare => {
                Some(Top::Function(function))
            }
            Declaration::VariableDeclaration(variables) if !variables.declare => {
                Some(Top::Variables(variables))
            }
            _ => None,
        }
    })
}

fn default_top<'a>(declaration: &'a ExportDefaultDeclarationKind<'a>) -> Option<Top<'a>> {
    match declaration {
        ExportDefaultDeclarationKind::ClassDeclaration(class) => Some(Top::Class(class)),
        ExportDefaultDeclarationKind::FunctionDeclaration(function) => {
            Some(Top::Function(function))
        }
        kind => match kind.as_expression().map(Expression::get_inner_expression) {
            Some(Expression::ClassExpression(class)) => Some(Top::Class(class)),
            Some(Expression::FunctionExpression(function)) => Some(Top::Function(function)),
            Some(Expression::ArrowFunctionExpression(arrow)) => Some(Top::Arrow(arrow)),
            _ => None,
        },
    }
}

/// From `class` or `abstract` to the end of the name, or of `class` without one.
fn class_head(file: &File<'_>, class: &Class<'_>) -> Span {
    let start = start_after_decorators(file, class.span, &class.decorators);
    let end = match &class.id {
        Some(id) => id.span.end,
        None if file.text[start as usize..].starts_with("abstract") => {
            skip_trivia(file.text, start + "abstract".len() as u32) + "class".len() as u32
        }
        None => start + "class".len() as u32,
    };
    Span::new(start, end)
}

/// Where a decorated class or class member starts without its decorators: past the last one and
/// the spaces and comments after it. Decorators before `export` lie outside the class's span
/// (`@D() export class A`) and leave its start; after it (`export @D() class B`) they lie inside.
pub fn start_after_decorators(file: &File<'_>, span: Span, decorators: &[Decorator<'_>]) -> u32 {
    match decorators.last() {
        Some(decorator) if decorator.span.end > span.start => {
            skip_trivia(file.text, decorator.span.end)
        }
        _ => span.start,
    }
}

/// The first offset from `at` that is not whitespace or a comment. Only for a stretch between
/// tokens, where `//` and `/*` can only open a comment.
fn skip_trivia(text: &str, at: u32) -> u32 {
    let mut rest = &text[at as usize..];
    loop {
        rest = rest.trim_start();
        if rest.starts_with("//") {
            rest = rest.find('\n').map_or("", |end| &rest[end..]);
        } else if rest.starts_with("/*") {
            rest = rest[2..].find("*/").map_or("", |end| &rest[end + 4..]);
        } else {
            return (text.len() - rest.len()) as u32;
        }
    }
}
