use kit::oxc::ast::ast::{Argument, Expression};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

struct Mocker {
    /// The names `modules` export it under.
    names: &'static [&'static str],
    modules: &'static [&'static str],
    /// The names tests see it under without an import.
    globals: &'static [&'static str],
    methods: &'static [&'static str],
}

const MOCKERS: &[Mocker] = &[
    Mocker {
        names: &["jest"],
        modules: &["@jest/globals"],
        globals: &["jest"],
        methods: &["mock", "doMock", "setMock", "unstable_mockModule"],
    },
    // Vitest gives `vi` as `vitest` too.
    Mocker {
        names: &["vi", "vitest"],
        modules: &["vitest"],
        globals: &["vi", "vitest"],
        methods: &["mock", "doMock"],
    },
    Mocker {
        names: &["mock"],
        modules: &["bun:test", "node:test"],
        globals: &[],
        methods: &["module"],
    },
];

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];
    const USES: &'static [Fact] = &[Fact::Packages, Fact::Aliases];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        let Some(member) = call.callee.get_member_expr() else {
            return;
        };
        let Some(method) = member.static_property_name() else {
            return;
        };
        let file = cx.file();
        let object = member.object().get_inner_expression();
        if !MOCKERS
            .iter()
            .any(|mocker| mocker.methods.contains(&method) && is_mocker(file, object, mocker))
        {
            return;
        }
        let Some(module) = call.arguments.first().and_then(Argument::as_expression) else {
            return;
        };
        let Some(source) = module_text(file, module) else {
            return;
        };
        if import_kind(file, cx.facts(), source).is_own() {
            // To the module, without the factory: a new fake keeps the fingerprint.
            cx.report(Span::new(call.span.start, module.span().end))
                .arg("call", one_line(call.callee.span().source_text(file.text)))
                .arg("module", source);
        }
    }
}

/// `jest`, `vi` as the runner gives them, or imported from the runner under any name.
fn is_mocker(file: &File<'_>, object: &Expression<'_>, mocker: &Mocker) -> bool {
    let Expression::Identifier(id) = object else {
        return false;
    };
    (mocker.globals.contains(&id.name.as_str()) && is_global(file, object, &id.name))
        || imported_name(file, id).is_some_and(|(module, name)| {
            mocker.names.contains(&name) && mocker.modules.contains(&package_name(module))
        })
}

/// The module a mock names; Vitest also takes `import('./x')`.
fn module_text<'a>(file: &File<'a>, module: &'a Expression<'a>) -> Option<&'a str> {
    match module {
        Expression::ImportExpression(import) => {
            module_source(file, AstKind::ImportExpression(import)).map(|s| s.value.as_str())
        }
        _ => static_text(module),
    }
}
