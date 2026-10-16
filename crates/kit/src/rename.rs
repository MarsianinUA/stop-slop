use std::iter;

use oxc_ast::ast::{
    BinaryOperator, Class, ClassElement, Expression, MethodDefinitionKind, PropertyKey, Statement,
    TSAccessibility, TSType, TSTypePredicateName,
};
use oxc_ast::{AstKind, AstType};
use oxc_semantic::{AstNode, AstNodes, NodeId, Reference, ScopeId, Scoping, SymbolFlags, SymbolId};
use oxc_span::{GetSpan, Span};

use crate::{Edit, File, is_exported};

/// Reserved words of strict mode and globals a binding must not shadow.
const RESERVED: &[&str] = &[
    "break",
    "case",
    "catch",
    "class",
    "const",
    "continue",
    "debugger",
    "default",
    "delete",
    "do",
    "else",
    "enum",
    "export",
    "extends",
    "false",
    "finally",
    "for",
    "function",
    "if",
    "import",
    "in",
    "instanceof",
    "new",
    "null",
    "return",
    "super",
    "switch",
    "this",
    "throw",
    "true",
    "try",
    "typeof",
    "var",
    "void",
    "while",
    "with",
    "let",
    "static",
    "yield",
    "await",
    "implements",
    "interface",
    "package",
    "private",
    "protected",
    "public",
    "arguments",
    "eval",
    "undefined",
    "NaN",
    "Infinity",
];

/// A type may not take these names (TS2457); a value may.
const BUILT_IN_TYPES: &[&str] = &[
    "any", "unknown", "never", "string", "number", "boolean", "symbol", "object", "bigint", "void",
    "null",
];

/// Edits that rename `symbol` to `name` in the whole file: every declaration (overloads and
/// merged declarations too), every reference and a parameter's name in its type predicate
/// (`value is Order`). A shorthand keeps its key: `{ value }` -> `{ value: name }`, in object
/// literals, destructuring and assignment patterns alike; and `{ name: value }` -> `{ name }`.
/// `None` when the rename may change what the code does or reach past this file.
pub fn rename(file: &File<'_>, symbol: SymbolId, name: &str) -> Option<Vec<Edit>> {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let old = scoping.symbol_name(symbol);
    if name == old
        || !is_identifier(name)
        || RESERVED.contains(&name)
        || (scoping.symbol_flags(symbol).intersects(SymbolFlags::Type)
            && BUILT_IN_TYPES.contains(&name))
        || has_direct_eval(scoping)
        || !is_own_name(file, symbol)
        || is_exported(file, symbol)
        || is_script_global(file, symbol)
        || is_decorated_class(file, symbol)
        || is_merged_type_parameter(file, symbol)
        || lets_runtime_name_out(file, symbol)
        || lowers_jsx_tag(file, symbol, name)
        || is_taken(file, symbol, name)
        || has_similar_name(file, symbol, name)
    {
        return None;
    }

    let mut edits: Vec<Edit> = occurrences(file, symbol)
        .map(|node| {
            // An escaped name (`\u0061`) is the same symbol under other text.
            (node.span().source_text(file.text) == old).then(|| edit(nodes, node, old, name))
        })
        .collect::<Option<_>>()?;
    edits.extend(predicate_name(file, symbol).map(|span| Edit {
        span,
        text: name.to_string(),
    }));
    Some(edits)
}

/// The parameter's name in the type predicate of its function (`value is Order`,
/// `asserts value`): a name, not a reference, so semantic leaves it out.
fn predicate_name(file: &File<'_>, symbol: SymbolId) -> Option<Span> {
    let nodes = file.semantic.nodes();
    let scoping = file.semantic.scoping();
    let declaration = scoping.symbol_declaration(symbol);
    let AstKind::FormalParameter(_) = nodes.kind(declaration) else {
        return None;
    };
    // The parent is the parameter list; its parent owns it.
    let return_type = match nodes.ancestor_kinds(declaration).nth(1)? {
        AstKind::Function(function) => function.return_type.as_deref(),
        AstKind::ArrowFunctionExpression(arrow) => arrow.return_type.as_deref(),
        AstKind::TSFunctionType(function) => Some(&*function.return_type),
        AstKind::TSMethodSignature(method) => method.return_type.as_deref(),
        AstKind::TSCallSignatureDeclaration(signature) => signature.return_type.as_deref(),
        _ => None,
    }?;
    match &return_type.type_annotation {
        TSType::TSTypePredicate(predicate) => match &predicate.parameter_name {
            TSTypePredicateName::Identifier(id)
                if id.span.source_text(file.text) == scoping.symbol_name(symbol) =>
            {
                Some(id.span)
            }
            _ => None,
        },
        _ => None,
    }
}

/// Every binding of `symbol` (overloads and merged declarations each bind it), then every
/// reference to it.
fn occurrences<'f, 'a>(
    file: &'f File<'a>,
    symbol: SymbolId,
) -> impl Iterator<Item = &'f AstNode<'a>> + 'f {
    let nodes = file.semantic.nodes();
    let bindings = nodes.iter().filter(move |node| {
        matches!(node.kind(), AstKind::BindingIdentifier(id) if id.symbol_id.get() == Some(symbol))
    });
    let references = file
        .semantic
        .scoping()
        .get_resolved_references(symbol)
        .map(|reference| nodes.get_node(reference.node_id()));
    bindings.chain(references)
}

/// The file chooses this name: a symbol it declares, not an import, an enum member, a
/// parameter property (a class member) or anything under `declare` (`declare const`,
/// a `declare function`'s parameters, `declare global`, `declare module`, `declare namespace`),
/// whose names come from elsewhere.
pub fn is_own_name(file: &File<'_>, symbol: SymbolId) -> bool {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    if scoping
        .symbol_flags(symbol)
        .intersects(SymbolFlags::Import | SymbolFlags::Ambient | SymbolFlags::EnumMember)
    {
        return false;
    }
    let declaration = scoping.symbol_declaration(symbol);
    let declares_own_name = match nodes.kind(declaration) {
        AstKind::FormalParameter(parameter) => !parameter.has_modifier(),
        AstKind::VariableDeclarator(_)
        | AstKind::Function(_)
        | AstKind::Class(_)
        | AstKind::FormalParameterRest(_)
        | AstKind::CatchParameter(_)
        | AstKind::TSInterfaceDeclaration(_)
        | AstKind::TSTypeAliasDeclaration(_)
        | AstKind::TSEnumDeclaration(_)
        | AstKind::TSTypeParameter(_)
        | AstKind::TSMappedType(_) => true,
        _ => false,
    };
    declares_own_name && !is_ambient(file, declaration)
}

/// The nodes [`private_member`] finds a member in, for a rule's `NODES`.
pub const PRIVATE_MEMBER_NODES: &[AstType] = &[
    AstType::PropertyDefinition,
    AstType::MethodDefinition,
    AstType::AccessorProperty,
    AstType::FormalParameter,
];

/// The name and span of a private class member: a `private` or `#` field, accessor or method
/// with a body (a method without one is an overload signature), or a `private` parameter
/// property; the `#` is not in the name. `None` for anything else: an interface, a base class,
/// a protocol (`toJSON`) or an outside format (`_id`) may dictate public and protected names.
/// A member is not a symbol: no rename reaches `this.x` here and `obj['x']` elsewhere, so a
/// finding on one has no fix.
pub fn private_member<'a>(file: &File<'a>, node: &AstNode<'a>) -> Option<(&'a str, Span)> {
    let member = match node.kind() {
        // A `declare` field only types a property that something else sets.
        AstKind::PropertyDefinition(property) if !property.declare => {
            private_key(property.accessibility, &property.key)
        }
        AstKind::AccessorProperty(accessor) => private_key(accessor.accessibility, &accessor.key),
        AstKind::MethodDefinition(method)
            if method.kind != MethodDefinitionKind::Constructor && method.value.body.is_some() =>
        {
            private_key(method.accessibility, &method.key)
        }
        AstKind::FormalParameter(parameter)
            if parameter.accessibility == Some(TSAccessibility::Private) =>
        {
            parameter
                .pattern
                .get_binding_identifier()
                .map(|id| (id.name.as_str(), id.span))
        }
        _ => None,
    }?;
    (!is_ambient(file, node.id())).then_some(member)
}

fn private_key<'a>(
    accessibility: Option<TSAccessibility>,
    key: &PropertyKey<'a>,
) -> Option<(&'a str, Span)> {
    match key {
        PropertyKey::PrivateIdentifier(id) => Some((id.name.as_str(), id.span)),
        PropertyKey::StaticIdentifier(id) if accessibility == Some(TSAccessibility::Private) => {
            Some((id.name.as_str(), id.span))
        }
        _ => None,
    }
}

/// The node or one around it is under `declare` (`declare class`, `declare module`,
/// `declare global`, ...): its code is elsewhere.
pub fn is_ambient(file: &File<'_>, node: NodeId) -> bool {
    let nodes = file.semantic.nodes();
    iter::once(nodes.kind(node))
        .chain(nodes.ancestor_kinds(node))
        .any(declares)
}

fn declares(kind: AstKind<'_>) -> bool {
    match kind {
        AstKind::TSGlobalDeclaration(_) | AstKind::TSExternalModuleDeclaration(_) => true,
        AstKind::Function(function) => function.declare,
        AstKind::Class(class) => class.declare,
        AstKind::VariableDeclaration(declaration) => declaration.declare,
        AstKind::TSNamespaceDeclaration(namespace) => namespace.declare,
        AstKind::TSEnumDeclaration(declaration) => declaration.declare,
        AstKind::TSInterfaceDeclaration(interface) => interface.declare,
        AstKind::TSTypeAliasDeclaration(alias) => alias.declare,
        _ => false,
    }
}

fn is_identifier(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|ch| ch.is_alphabetic() || ch == '_' || ch == '$')
        && chars.all(|ch| ch.is_alphanumeric() || ch == '_' || ch == '$')
}

/// A file without `import` or `export` is a script: its top-level names are global to other
/// files.
pub fn is_script_global(file: &File<'_>, symbol: SymbolId) -> bool {
    let scoping = file.semantic.scoping();
    scoping.symbol_scope_id(symbol) == scoping.root_scope_id()
        && !file
            .semantic
            .nodes()
            .program()
            .body
            .iter()
            .any(Statement::is_module_declaration)
}

/// Direct `eval` looks names up at run time. oxc marks every scope around one, the root too.
fn has_direct_eval(scoping: &Scoping) -> bool {
    scoping.root_scope_flags().contains_direct_eval()
}

/// A decorator gets the whole class, and frameworks (Nest, TypeORM) read its `.name`.
fn is_decorated_class(file: &File<'_>, symbol: SymbolId) -> bool {
    let nodes = file.semantic.nodes();
    file.semantic
        .scoping()
        .symbol_declarations(symbol)
        .any(|declaration| match nodes.kind(declaration) {
            AstKind::Class(class) => {
                !class.decorators.is_empty() || class.body.body.iter().any(has_decorator)
            }
            _ => false,
        })
}

/// Every declaration of a merged interface or class repeats the type parameters, and TS wants
/// them the same (TS2428), but each declares its own symbol.
fn is_merged_type_parameter(file: &File<'_>, symbol: SymbolId) -> bool {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let declaration = scoping.symbol_declaration(symbol);
    let AstKind::TSTypeParameter(_) = nodes.kind(declaration) else {
        return false;
    };
    // The parent is the `<..>` list; its parent owns it.
    let owner = match nodes.ancestor_kinds(declaration).nth(1) {
        Some(AstKind::TSInterfaceDeclaration(interface)) => Some(&interface.id),
        Some(AstKind::Class(class)) => class.id.as_ref(),
        _ => None,
    };
    owner
        .and_then(|id| id.symbol_id.get())
        .is_some_and(|owner| !scoping.symbol_redeclarations(owner).is_empty())
}

/// oxc's `has_decorator` skips parameter decorators (`m(@Inject() x)`).
fn has_decorator(element: &ClassElement<'_>) -> bool {
    element.has_decorator()
        || matches!(element, ClassElement::MethodDefinition(method)
            if method.value.params.items.iter().any(|parameter| !parameter.decorators.is_empty()))
}

/// The value's `.name` is the symbol's name (logs, `new Logger(X.name)`, DI) and some reference
/// lets the value go: a function or class, or a symbol that a function, arrow or class is
/// assigned to directly. The name of a function or class expression always goes with the
/// expression, and a class that reads its own name goes with it too.
fn lets_runtime_name_out(file: &File<'_>, symbol: SymbolId) -> bool {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let mut named_value = false;
    for declaration in scoping.symbol_declarations(symbol) {
        match nodes.kind(declaration) {
            AstKind::Function(function) if function.is_expression() => return true,
            AstKind::Class(class) if class.is_expression() || reads_own_name(nodes, class) => {
                return true;
            }
            AstKind::Function(_) | AstKind::Class(_) => named_value = true,
            _ => {}
        }
    }
    (named_value || occurrences(file, symbol).any(|node| names_its_value(nodes, node)))
        && !scoping
            .get_resolved_references(symbol)
            .all(|reference| is_call_or_type(nodes, reference))
}

/// `this.constructor.name`, `new.target.name`.
fn reads_own_name(nodes: &AstNodes<'_>, class: &Class<'_>) -> bool {
    nodes.iter().any(|node| match node.kind() {
        AstKind::StaticMemberExpression(member) => {
            class.span.contains_inclusive(member.span)
                && match &member.object {
                    Expression::NewTarget(_) => true,
                    Expression::ThisExpression(_) => member.property.name == "constructor",
                    _ => false,
                }
        }
        _ => false,
    })
}

/// A function, arrow or class assigned straight to this binding or reference takes its name:
/// `const f = () => {}`, `f = () => {}`, `(f = () => {}) => ..`, `{ f = () => {} }`.
fn names_its_value(nodes: &AstNodes<'_>, node: &AstNode<'_>) -> bool {
    let span = node.span();
    let value = match nodes.parent_kind(node.id()) {
        AstKind::VariableDeclarator(declarator) if declarator.id.span() == span => {
            declarator.init.as_ref()
        }
        AstKind::FormalParameter(parameter) => parameter.initializer.as_deref(),
        AstKind::AssignmentPattern(pattern) if pattern.left.span() == span => Some(&pattern.right),
        AstKind::AssignmentExpression(assignment) if assignment.left.span() == span => {
            Some(&assignment.right)
        }
        AstKind::AssignmentTargetWithDefault(target) if target.binding.span() == span => {
            Some(&target.init)
        }
        _ => None,
    };
    value.is_some_and(|value| {
        matches!(
            value.get_inner_expression(),
            Expression::FunctionExpression(_)
                | Expression::ArrowFunctionExpression(_)
                | Expression::ClassExpression(_)
        )
    })
}

/// `f()`, `new F()`, `x instanceof F` or a type position (`: F`, `typeof f`).
fn is_call_or_type(nodes: &AstNodes<'_>, reference: &Reference) -> bool {
    if reference.is_type() {
        return true;
    }
    let span = nodes.get_node(reference.node_id()).span();
    match nodes.parent_kind(reference.node_id()) {
        AstKind::CallExpression(call) => call.callee.span() == span,
        AstKind::NewExpression(new) => new.callee.span() == span,
        AstKind::BinaryExpression(binary) => {
            binary.operator == BinaryOperator::Instanceof && binary.right.span() == span
        }
        AstKind::TSTypeQuery(_) | AstKind::TSQualifiedName(_) => true,
        _ => false,
    }
}

/// A JSX tag names a component only while it starts with no lowercase letter: `<icon />` is an
/// HTML element, so renaming the symbol of `<Icon />` to `icon` changes what renders.
fn lowers_jsx_tag(file: &File<'_>, symbol: SymbolId, name: &str) -> bool {
    name.starts_with(|ch: char| ch.is_ascii_lowercase()) && is_jsx_tag(file, symbol)
}

/// A JSX tag refers to the symbol: `<Icon />`.
pub fn is_jsx_tag(file: &File<'_>, symbol: SymbolId) -> bool {
    let nodes = file.semantic.nodes();
    file.semantic
        .scoping()
        .get_resolved_references(symbol)
        .any(|reference| {
            matches!(
                nodes.parent_kind(reference.node_id()),
                AstKind::JSXOpeningElement(_) | AstKind::JSXClosingElement(_)
            )
        })
}

/// `name` is bound in the symbol's scope or a scope inside it, or some reference there already
/// means `name`. Deliberately coarse: no shadowing and no capture, at the cost of a rare finding
/// without a fix.
fn is_taken(file: &File<'_>, symbol: SymbolId, name: &str) -> bool {
    let scoping = file.semantic.scoping();
    let home = scoping.symbol_scope_id(symbol);
    let inside = |scope| is_within(scoping, scope, home);
    // A `var` written inside binds in the function scope above: `catch (e) { var error = e; }`
    // renamed to `error` would assign the catch parameter instead.
    scoping
        .iter_bindings()
        .any(|(scope, bindings)| bindings.contains_key(name) && inside(scope))
        || file.semantic.nodes().iter().any(|node| {
            let named = match node.kind() {
                AstKind::IdentifierReference(id) => id.name == name,
                AstKind::BindingIdentifier(id) => id.name == name,
                _ => false,
            };
            named && inside(node.scope_id())
        })
}

/// Another value (or type) in a related scope whose name differs from `name` only in `_`, `-`,
/// `$` and case (`user_id` while `userID` becomes `userId`): another rule renames it in the
/// same pass, and the two names would become one.
fn has_similar_name(file: &File<'_>, symbol: SymbolId, name: &str) -> bool {
    let scoping = file.semantic.scoping();
    let flags = scoping.symbol_flags(symbol);
    related_symbols(file, symbol).any(|other| {
        let other_flags = scoping.symbol_flags(other);
        let both = |meaning| flags.intersects(meaning) && other_flags.intersects(meaning);
        let other_name = scoping.symbol_name(other);
        (both(SymbolFlags::Value) || both(SymbolFlags::Type))
            && other_name != name
            && is_similar_name(other_name, name)
    })
}

/// The names differ only in `_`, `-`, `$` and case: `user_id`, `userID` and `userId`.
pub fn is_similar_name(a: &str, b: &str) -> bool {
    fn loose(name: &str) -> impl Iterator<Item = char> + '_ {
        name.chars()
            .filter(|ch| !matches!(ch, '_' | '-' | '$'))
            .flat_map(char::to_lowercase)
    }
    loose(a).eq(loose(b))
}

/// The file's other symbols whose scope is the symbol's, inside it or around it: the names
/// that one pass of renames in the symbol's scope may meet.
pub fn related_symbols<'a>(
    file: &File<'a>,
    symbol: SymbolId,
) -> impl Iterator<Item = SymbolId> + 'a {
    let scoping = file.semantic.scoping();
    let home = scoping.symbol_scope_id(symbol);
    scoping.symbol_ids().filter(move |&other| {
        let scope = scoping.symbol_scope_id(other);
        other != symbol && (is_within(scoping, scope, home) || is_within(scoping, home, scope))
    })
}

/// `scope` is `outer` or nested in it.
fn is_within(scoping: &Scoping, scope: ScopeId, outer: ScopeId) -> bool {
    scoping
        .scope_ancestors(scope)
        .any(|ancestor| ancestor == outer)
}

/// Renames the binding or reference `node`; in an object the key stays, so the object keeps its
/// shape: `{ value }` -> `{ value: name }`, and `{ name: value }` -> `{ name }`.
fn edit(nodes: &AstNodes<'_>, node: &AstNode<'_>, old: &str, name: &str) -> Edit {
    let span = node.span();
    let mut parent = nodes.parent_node(node.id());
    let has_default = match parent.kind() {
        AstKind::AssignmentPattern(pattern) => pattern.left.span() == span,
        AstKind::AssignmentTargetWithDefault(target) => target.binding.span() == span,
        _ => false,
    };
    if has_default {
        parent = nodes.parent_node(parent.id());
    }
    let keep_key = || Edit {
        span,
        text: format!("{old}: {name}"),
    };
    let drop_key = |key: Span| Edit {
        span: Span::new(key.start, span.end),
        text: name.to_string(),
    };
    match parent.kind() {
        AstKind::BindingProperty(property) if property.shorthand => keep_key(),
        AstKind::ObjectProperty(property) if property.shorthand => keep_key(),
        AstKind::AssignmentTargetPropertyIdentifier(property) if property.binding.span == span => {
            keep_key()
        }
        AstKind::BindingProperty(property)
            if !property.computed && property.key.is_specific_static_name(name) =>
        {
            drop_key(property.key.span())
        }
        AstKind::ObjectProperty(property)
            if !property.computed
                && property.value.span() == span
                && property.key.is_specific_static_name(name) =>
        {
            drop_key(property.key.span())
        }
        AstKind::AssignmentTargetPropertyProperty(property)
            if !property.computed && property.name.is_specific_static_name(name) =>
        {
            drop_key(property.name.span())
        }
        _ => Edit {
            span,
            text: name.to_string(),
        },
    }
}
