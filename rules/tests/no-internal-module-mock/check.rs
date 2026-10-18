use kit::oxc::ast::ast::Expression;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

/// A test runner's object that swaps a module for a fake: its name, the module that exports it,
/// whether tests see it as a global too, and the methods that register the fake.
struct Mocker {
    object: &'static str,
    module: &'static str,
    global: bool,
    methods: &'static [&'static str],
}

const MOCKERS: [Mocker; 4] = [
    Mocker {
        object: "jest",
        module: "@jest/globals",
        global: true,
        methods: &["mock", "doMock", "setMock", "unstable_mockModule"],
    },
    Mocker {
        object: "vi",
        module: "vitest",
        global: true,
        methods: &["mock", "doMock"],
    },
    Mocker {
        object: "mock",
        module: "bun:test",
        global: false,
        methods: &["module"],
    },
    Mocker {
        object: "mock",
        module: "node:test",
        global: false,
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
        let Some(module) = call
            .arguments
            .first()
            .and_then(|first| first.as_expression())
        else {
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
    (mocker.global && is_global(file, object, mocker.object))
        || imported_from(file, id, mocker.module) == Some(mocker.object)
}

/// The module a mock names: a string, a template without `${}`, or Vitest's `import('./x')`.
fn module_text<'a>(file: &File<'a>, module: &'a Expression<'a>) -> Option<&'a str> {
    match module {
        Expression::StringLiteral(literal) => Some(literal.value.as_str()),
        Expression::TemplateLiteral(template) => template.single_quasi().map(|text| text.as_str()),
        Expression::ImportExpression(import) => {
            module_source(file, AstKind::ImportExpression(import)).map(|s| s.value.as_str())
        }
        _ => None,
    }
}
