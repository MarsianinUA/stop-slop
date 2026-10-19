use kit::oxc::ast::ast::{
    Argument, ArrayExpressionElement, BindingPattern, CallExpression, Class, ClassElement,
    Expression, FunctionBody, IdentifierReference, LogicalOperator, Statement,
};
use kit::oxc::semantic::{NodeId, SymbolId};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields, rename_all = "kebab-case")]
pub struct Check {
    min_literal_items: usize,
}

const LOOKUPS: [&str; 6] = ["includes", "indexOf", "find", "findIndex", "some", "filter"];

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Some(member) = call.callee.get_member_expr() else {
            return;
        };
        let Some(method) = member
            .static_property_name()
            .filter(|method| LOOKUPS.contains(method))
        else {
            return;
        };
        let file = cx.file();
        let Some(array) = scanned(file, member.object(), node.id()) else {
            return;
        };
        let Some(key) = key(file, method, call) else {
            return;
        };
        let Some(around) = loop_around(file, node.id()) else {
            return;
        };
        // A listener runs per event, not per item of a collection. A search of the loop's own
        // collection (`ids.filter((id, i) => ids.indexOf(id) === i)`) has no other side to index.
        if around.kind == LoopKind::Listener || around.over.is_some_and(|over| array.is(file, over))
        {
            return;
        }
        let pass = around.node.span();
        let nodes = file.semantic.nodes();
        let uses = array.uses(file);
        // An array built on each pass belongs to the item; one the loop changes cannot be indexed
        // once before it.
        let is_changed_in_loop = uses
            .iter()
            .any(|&id| pass.contains_inclusive(nodes.get_node(id).span()) && changes(file, id));
        if array.is_declared_in(file, pass) || is_changed_in_loop {
            return;
        }
        let Some(origin) = origin(file, &array, &uses, self.min_literal_items) else {
            return;
        };
        cx.report(Span::new(call.span.start, member.span().end))
            .arg(
                "array",
                one_line(member.object().span().source_text(file.text)),
            )
            .arg("origin", origin)
            .arg("method", method)
            .arg("loop", around.label(file))
            .arg("depth", around.depth)
            .arg("key", key);
    }
}

/// The array a search scans: a name of the file, or a field of the class whose `this` the call
/// sees.
enum Scanned<'a> {
    Name(SymbolId),
    Field(&'a Class<'a>, &'a str),
}

/// `ids` of `ids.includes(..)` when its syntax shows an array (`kit::is_known_array`), or
/// `this.blocked` when the class starts the field as one. Another call's result is not seen: it
/// may be built on each pass.
fn scanned<'a>(file: &File<'a>, receiver: &'a Expression<'a>, call: NodeId) -> Option<Scanned<'a>> {
    match receiver.get_inner_expression() {
        Expression::Identifier(id) if is_known_array(file, receiver) => file
            .semantic
            .scoping()
            .get_reference(id.reference_id())
            .symbol_id()
            .map(Scanned::Name),
        Expression::StaticMemberExpression(member)
            if matches!(member.object, Expression::ThisExpression(_)) =>
        {
            let class = this_class(file, call)?;
            let name = member.property.name.as_str();
            field_value(class, name)
                .is_some_and(|value| is_known_array(file, value))
                .then_some(Scanned::Field(class, name))
        }
        _ => None,
    }
}

impl Scanned<'_> {
    /// `expression` reads this array: the same name, or the same field of `this`.
    fn is(&self, file: &File<'_>, expression: &Expression<'_>) -> bool {
        match (self, expression.get_inner_expression()) {
            (Scanned::Name(symbol), Expression::Identifier(id)) => {
                file.semantic
                    .scoping()
                    .get_reference(id.reference_id())
                    .symbol_id()
                    == Some(*symbol)
            }
            (Scanned::Field(_, name), Expression::StaticMemberExpression(member)) => {
                matches!(member.object, Expression::ThisExpression(_))
                    && member.property.name.as_str() == *name
            }
            _ => false,
        }
    }

    fn is_declared_in(&self, file: &File<'_>, span: Span) -> bool {
        match self {
            Scanned::Name(symbol) => {
                span.contains_inclusive(file.semantic.scoping().symbol_span(*symbol))
            }
            Scanned::Field(..) => false,
        }
    }

    /// The nodes that read the array: the references of the name, or `this.<field>` in the
    /// methods of the class.
    fn uses(&self, file: &File<'_>) -> Vec<NodeId> {
        match self {
            Scanned::Name(symbol) => file
                .semantic
                .scoping()
                .get_resolved_references(*symbol)
                .map(|reference| reference.node_id())
                .collect(),
            Scanned::Field(class, name) => file
                .semantic
                .nodes()
                .iter()
                .filter(|node| match node.kind() {
                    AstKind::StaticMemberExpression(member) => {
                        matches!(member.object, Expression::ThisExpression(_))
                            && member.property.name.as_str() == *name
                            && this_class(file, node.id())
                                .is_some_and(|other| other.span == class.span)
                    }
                    _ => false,
                })
                .map(AstNode::id)
                .collect(),
        }
    }
}

/// The value a non-static field starts with: `[]` of `private readonly blocked: string[] = []`.
fn field_value<'a>(class: &'a Class<'a>, name: &str) -> Option<&'a Expression<'a>> {
    class.body.body.iter().find_map(|element| match element {
        ClassElement::PropertyDefinition(field)
            if !field.r#static && field.key.static_name().is_some_and(|key| key == name) =>
        {
            field.value.as_ref()
        }
        _ => None,
    })
}

/// The use changes the array: `ids.push(..)`, `ids[0] = ..`, `ids.length = 0`, `ids = [..]`.
fn changes(file: &File<'_>, id: NodeId) -> bool {
    let nodes = file.semantic.nodes();
    let node = outer_assertion(file, nodes.get_node(id));
    let is_assigned = matches!(nodes.parent_kind(node.id()), AstKind::AssignmentExpression(assignment)
        if assignment.left.span() == node.span());
    is_assigned || mutator_call(file, id).is_some() || member_write(file, id).is_some()
}

/// Where the array comes from, for the message. `None` for a literal of fewer than `min` items
/// that nothing changes: a fixed list of options, not data.
fn origin(file: &File<'_>, array: &Scanned<'_>, uses: &[NodeId], min: usize) -> Option<String> {
    let (holder, value) = match *array {
        Scanned::Field(class, name) => ("a field of the class", field_value(class, name)),
        Scanned::Name(symbol) => {
            let declaration = file.semantic.scoping().symbol_declaration(symbol);
            match file.semantic.nodes().kind(declaration) {
                AstKind::VariableDeclarator(declarator) => ("a variable", declarator.init.as_ref()),
                _ => ("a parameter", None),
            }
        }
    };
    let value = value.map(Expression::get_inner_expression);
    if let Some(Expression::ArrayExpression(literal)) = value {
        let is_fixed = !literal
            .elements
            .iter()
            .any(|element| matches!(element, ArrayExpressionElement::SpreadElement(_)))
            && !uses.iter().any(|&id| changes(file, id));
        if !is_fixed {
            return Some(holder.to_string());
        }
        let items = literal.elements.len();
        return (items >= min).then(|| format!("{holder} set to a literal of {items} items"));
    }
    Some(match value.and_then(call_of) {
        Some(call) => format!(
            "{holder} set to the result of `{}(..)`",
            one_line(call.callee.span().source_text(file.text))
        ),
        None => holder.to_string(),
    })
}

/// What the search matches items by: `value` for `includes` and `indexOf` and for a callback that
/// compares its item (`(id) => id === wanted`); `.id` for one that compares a member of it
/// (`(user) => user.id === id`, `({ id }) => id === wanted`), also in a clause of `&&`. `None`
/// for any other callback: no index replaces a scan by a predicate.
fn key(file: &File<'_>, method: &str, call: &CallExpression<'_>) -> Option<String> {
    if matches!(method, "includes" | "indexOf") {
        return Some("value".to_string());
    }
    let (params, result) = match call
        .arguments
        .first()
        .and_then(Argument::as_expression)?
        .get_inner_expression()
    {
        Expression::ArrowFunctionExpression(arrow) => {
            let result = match arrow.get_expression() {
                Some(expression) => expression,
                None => returned(arrow.get_function_body()?)?,
            };
            (&arrow.params, result)
        }
        Expression::FunctionExpression(function) => {
            (&function.params, returned(function.body.as_deref()?)?)
        }
        _ => return None,
    };
    let item = bindings(&params.items.first()?.pattern);
    compared(file, &item, result)
}

/// The value of a body that is one `return`.
fn returned<'b, 'a>(body: &'b FunctionBody<'a>) -> Option<&'b Expression<'a>> {
    match body.statements.as_slice() {
        [Statement::ReturnStatement(statement)] => statement.argument.as_ref(),
        _ => None,
    }
}

/// The names the item parameter binds and the member each stands for: `user` of `(user) =>` is
/// the item itself (`None`), `id` of `({ id }) =>` is `.id`.
fn bindings(pattern: &BindingPattern<'_>) -> Vec<(SymbolId, Option<String>)> {
    match pattern {
        BindingPattern::BindingIdentifier(id) => id
            .symbol_id
            .get()
            .map(|symbol| (symbol, None))
            .into_iter()
            .collect(),
        BindingPattern::ObjectPattern(object) => object
            .properties
            .iter()
            .filter_map(|property| {
                let BindingPattern::BindingIdentifier(id) = &property.value else {
                    return None;
                };
                let key = property.key.static_name()?.into_owned();
                Some((id.symbol_id.get()?, Some(key)))
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn compared(
    file: &File<'_>,
    item: &[(SymbolId, Option<String>)],
    test: &Expression<'_>,
) -> Option<String> {
    match test.get_inner_expression() {
        Expression::LogicalExpression(logical) if logical.operator == LogicalOperator::And => {
            compared(file, item, &logical.left).or_else(|| compared(file, item, &logical.right))
        }
        Expression::BinaryExpression(binary) if binary.operator.is_equality() => {
            matched(file, item, &binary.left, &binary.right)
                .or_else(|| matched(file, item, &binary.right, &binary.left))
        }
        _ => None,
    }
}

/// The key when `side` is the item or a member of it and `other` does not read the item:
/// `user.id === order.userId` matches by `.id`, `user.id === user.parentId` by nothing.
fn matched(
    file: &File<'_>,
    item: &[(SymbolId, Option<String>)],
    side: &Expression<'_>,
    other: &Expression<'_>,
) -> Option<String> {
    let scoping = file.semantic.scoping();
    let nodes = file.semantic.nodes();
    let reads_item = item.iter().any(|(symbol, _)| {
        scoping.get_resolved_references(*symbol).any(|reference| {
            other
                .span()
                .contains_inclusive(nodes.get_node(reference.node_id()).span())
        })
    });
    if reads_item {
        return None;
    }
    let binding = |id: &IdentifierReference<'_>| {
        let symbol = scoping.get_reference(id.reference_id()).symbol_id()?;
        item.iter().find(|(bound, _)| *bound == symbol)
    };
    match side.get_inner_expression() {
        Expression::Identifier(id) => Some(match &binding(id)?.1 {
            Some(key) => format!("`.{key}`"),
            None => "value".to_string(),
        }),
        side => {
            let member = side.get_member_expr()?;
            let Expression::Identifier(id) = member.object().get_inner_expression() else {
                return None;
            };
            // `id.length` of `({ id }) =>` is not a member of the item.
            let (_, None) = binding(id)? else {
                return None;
            };
            Some(format!("`.{}`", member.static_property_name()?))
        }
    }
}
