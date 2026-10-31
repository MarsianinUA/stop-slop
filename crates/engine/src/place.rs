use std::borrow::Cow;
use std::iter;

use kit::oxc::ast::ast::{
    BindingPattern, Declaration, ExportDefaultDeclarationKind, Expression, PropertyKey, Statement,
};
use kit::oxc::semantic::{AstNodes, NodeId, Semantic};
use kit::oxc::span::{GetSpan, Span};
use kit::prelude::AstKind;
use kit::{LineIndex, line_start, one_line, skip_trivia};

use crate::Changed;

const ANONYMOUS: &str = "(anonymous)";
/// Code outside any declaration: an import, a call at the top level.
const TOP_LEVEL: &str = "(top level)";

/// A function, a class member or a top-level statement, and the name the code gives it.
struct Unit {
    span: Span,
    name: String,
}

/// The code that findings of a file belong to, for a review that asks whether the diff
/// touched it.
pub(crate) struct Places<'t> {
    text: &'t str,
    /// Functions and arrows, and class members that hold no function. A function spans what
    /// names it: a method its decorators and key, an arrow its variable, so a finding on the
    /// name belongs to the function.
    inner: Vec<Unit>,
    /// The statements of the program.
    top: Vec<Unit>,
    /// In order.
    comments: Vec<Span>,
    /// Whether each line, from 1, holds code and not only spaces and comments; empty without a
    /// diff to compare with.
    code: Vec<bool>,
}

impl<'t> Places<'t> {
    pub(crate) fn new(semantic: &Semantic<'_>, text: &'t str, with_diff: bool) -> Places<'t> {
        let nodes = semantic.nodes();
        let mut inner = Vec::new();
        for node in nodes.iter() {
            let kind = node.kind();
            if matches!(
                kind,
                AstKind::Function(_) | AstKind::ArrowFunctionExpression(_)
            ) {
                inner.push(function_unit(nodes, node.id(), text));
            } else if let Some((span, key, value)) = field(kind)
                // A field that holds a function is that function's unit.
                && !holds_function(value)
            {
                let name = member_name(nodes, node.id(), key, text);
                inner.push(Unit { span, name });
            }
        }
        let top = nodes
            .program()
            .body
            .iter()
            .map(|statement| Unit {
                span: statement.span(),
                name: statement_name(statement, text),
            })
            .collect();
        let comments: Vec<Span> = semantic
            .comments()
            .iter()
            .map(|comment| comment.span)
            .collect();
        let code = if with_diff {
            code_lines(text, &comments)
        } else {
            Vec::new()
        };
        Places {
            text,
            inner,
            top,
            comments,
            code,
        }
    }

    /// The name of the code around the start of `span`, and whether `changed` touched it: the
    /// innermost function or class member, else the top-level statement, else the lines of
    /// `span` alone. A finding in a comment belongs to the code the comment is about.
    pub(crate) fn locate(
        &self,
        span: Span,
        lines: &LineIndex,
        changed: Option<&Changed>,
    ) -> (String, Option<bool>) {
        let at = self.subject(span.start);
        let (name, reach) = match around(&self.inner, at).or_else(|| around(&self.top, at)) {
            Some(unit) => (unit.name.clone(), unit.span),
            None => (TOP_LEVEL.to_owned(), span),
        };
        let touched = changed.map(|changed| {
            let first = lines.position(reach.start).line;
            self.touched(changed, first, lines.position(reach.end).line)
        });
        (name, touched)
    }

    /// For an offset in a comment, the code the comment is about: the code before it on its
    /// line (`total(); // why`), else the code after it (a doc comment above a method).
    fn subject(&self, offset: u32) -> u32 {
        let index = self
            .comments
            .partition_point(|comment| comment.end <= offset);
        let Some(comment) = self
            .comments
            .get(index)
            .filter(|comment| comment.start <= offset)
        else {
            return offset;
        };
        let line_start = line_start(self.text, comment.start) as usize;
        let before = self.text[line_start..comment.start as usize].trim_end();
        if !before.is_empty() {
            return (line_start + before.len() - 1) as u32;
        }
        let at = skip_trivia(self.text, comment.end);
        if (at as usize) < self.text.len() {
            at
        } else {
            offset
        }
    }

    /// A changed line of code in `first..=last`, or a hunk that took code out there.
    fn touched(&self, changed: &Changed, first: u32, last: u32) -> bool {
        let is_code = |line: u32| self.code.get(line as usize).copied().unwrap_or(false);
        let edited = changed
            .lines
            .iter()
            .any(|range| (range.start.max(first)..=range.end.min(last)).any(is_code));
        edited
            || changed.cuts.iter().any(|cut| match cut.count {
                // Between line `start` and the next.
                0 => first <= cut.start && cut.start < last,
                count => cut.start <= last && first < cut.start.saturating_add(count),
            })
    }
}

/// The innermost of `units` around `offset`.
fn around(units: &[Unit], offset: u32) -> Option<&Unit> {
    units
        .iter()
        .filter(|unit| unit.span.start <= offset && offset < unit.span.end)
        .min_by_key(|unit| unit.span.size())
}

/// Index 0 stands for no line.
fn code_lines(text: &str, comments: &[Span]) -> Vec<bool> {
    let mut code = vec![false, false];
    let mut comments = comments.iter().peekable();
    let bytes = text.as_bytes();
    let mut at = 0;
    while at < bytes.len() {
        if let Some(comment) = comments.next_if(|comment| comment.start as usize <= at) {
            let end = (comment.end as usize).clamp(at, bytes.len());
            let breaks = bytes[at..end].iter().filter(|&&byte| byte == b'\n').count();
            code.extend(iter::repeat_n(false, breaks));
            at = end;
            continue;
        }
        match bytes[at] {
            b'\n' => code.push(false),
            b' ' | b'\t' | b'\r' | 0x0b | 0x0c => {}
            _ => *code.last_mut().expect("line 1 is there") = true,
        }
        at += 1;
    }
    code
}

fn function_unit(nodes: &AstNodes<'_>, id: NodeId, text: &str) -> Unit {
    let own = nodes.get_node(id);
    let own_name = match own.kind() {
        AstKind::Function(function) => function.id.as_ref().map(|id| id.name.to_string()),
        _ => None,
    };
    let (holder_id, holder) = holder(nodes, id);
    let (span, name) = match holder {
        AstKind::MethodDefinition(method) => (
            method.span,
            member_name(nodes, holder_id, &method.key, text),
        ),
        AstKind::ObjectProperty(property) => (property.span, key_text(&property.key, text)),
        AstKind::VariableDeclarator(declarator) => {
            (declarator.span, binding_text(&declarator.id, text))
        }
        AstKind::AssignmentExpression(assignment) => (
            assignment.span,
            one_line(assignment.left.span().source_text(text)),
        ),
        AstKind::ExportDeclaration(_) | AstKind::ExportDefaultDeclaration(_) => {
            (holder.span(), anonymous(own_name))
        }
        kind => match field(kind) {
            Some((span, key, _)) => (span, member_name(nodes, holder_id, key, text)),
            None => (own.span(), anonymous(own_name)),
        },
    };
    Unit { span, name }
}

/// A class field, plain or `accessor`: its span, key and value.
fn field<'a>(kind: AstKind<'a>) -> Option<(Span, &'a PropertyKey<'a>, Option<&'a Expression<'a>>)> {
    match kind {
        AstKind::PropertyDefinition(field) => Some((field.span, &field.key, field.value.as_ref())),
        AstKind::AccessorProperty(accessor) => {
            Some((accessor.span, &accessor.key, accessor.value.as_ref()))
        }
        _ => None,
    }
}

fn holds_function(value: Option<&Expression<'_>>) -> bool {
    value.is_some_and(|value| value.get_inner_expression().is_function())
}

fn anonymous(name: Option<String>) -> String {
    name.unwrap_or_else(|| ANONYMOUS.to_owned())
}

/// The node that holds the function or class at `id`, past type assertions. Parentheses are
/// already gone: the engine parses without `preserve_parens`.
fn holder<'a>(nodes: &AstNodes<'a>, mut id: NodeId) -> (NodeId, AstKind<'a>) {
    loop {
        let parent = nodes.parent_id(id);
        match nodes.kind(parent) {
            AstKind::TSAsExpression(_)
            | AstKind::TSSatisfiesExpression(_)
            | AstKind::TSNonNullExpression(_)
            | AstKind::TSTypeAssertion(_) => id = parent,
            kind => return (parent, kind),
        }
    }
}

/// `Class.key` for the member at `id`.
fn member_name(nodes: &AstNodes<'_>, id: NodeId, key: &PropertyKey<'_>, text: &str) -> String {
    let class = nodes
        .ancestor_ids(id)
        .find_map(|ancestor| match nodes.kind(ancestor) {
            AstKind::Class(class) => Some((ancestor, class)),
            _ => None,
        });
    let class = class.and_then(|(class_id, class)| match &class.id {
        Some(id) => Some(id.name.to_string()),
        // `const Orders = class { .. }`
        None => match holder(nodes, class_id).1 {
            AstKind::VariableDeclarator(declarator) => Some(binding_text(&declarator.id, text)),
            _ => None,
        },
    });
    format!("{}.{}", anonymous(class), key_text(key, text))
}

/// `#name` for a private key, `[expression]` for a computed one without a static name.
fn key_text(key: &PropertyKey<'_>, text: &str) -> String {
    match key {
        PropertyKey::PrivateIdentifier(id) => format!("#{}", id.name),
        _ => key.static_name().map_or_else(
            || format!("[{}]", one_line(key.span().source_text(text))),
            Cow::into_owned,
        ),
    }
}

/// A pattern such as `{ a, b }` as the code writes it.
fn binding_text(pattern: &BindingPattern<'_>, text: &str) -> String {
    match pattern.get_identifier_name() {
        Some(name) => name.to_string(),
        None => one_line(pattern.span().source_text(text)),
    }
}

fn statement_name(statement: &Statement<'_>, text: &str) -> String {
    let declaration = match statement {
        Statement::ExportDeclaration(export) => Some(&export.declaration),
        Statement::ExportDefaultDeclaration(export) => {
            let id = match &export.declaration {
                ExportDefaultDeclarationKind::FunctionDeclaration(function) => function.id.as_ref(),
                ExportDefaultDeclarationKind::ClassDeclaration(class) => class.id.as_ref(),
                ExportDefaultDeclarationKind::TSInterfaceDeclaration(interface) => {
                    Some(&interface.id)
                }
                _ => None,
            };
            return id.map_or_else(|| "default".to_owned(), |id| id.name.to_string());
        }
        _ => statement.as_declaration(),
    };
    match declaration {
        Some(Declaration::VariableDeclaration(variables)) => variables
            .declarations
            .iter()
            .map(|declarator| binding_text(&declarator.id, text))
            .collect::<Vec<_>>()
            .join(", "),
        Some(Declaration::TSExternalModuleDeclaration(module)) => {
            module.id.span.source_text(text).to_owned()
        }
        Some(Declaration::TSGlobalDeclaration(_)) => "global".to_owned(),
        Some(declaration) => declaration
            .id()
            .map_or_else(|| TOP_LEVEL.to_owned(), |id| id.name.to_string()),
        None => TOP_LEVEL.to_owned(),
    }
}
