use kit::EnumMember;
use kit::oxc::ast::ast::BinaryOperator;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {}

impl Rule for Check {
    const USES: &'static [Fact] = &[Fact::Enums];
    const NODES: &'static [AstType] = &[AstType::StringLiteral, AstType::TemplateLiteral];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let span = node.span();
        if !is_value(file.semantic.nodes().parent_kind(node.id()), span) {
            return;
        }
        let value = match node.kind() {
            AstKind::StringLiteral(literal) => literal.value.as_str(),
            AstKind::TemplateLiteral(template) => match template.single_quasi() {
                Some(value) => value.as_str(),
                None => return,
            },
            _ => return,
        };
        let mut members: Vec<String> = Vec::new();
        for member in cx.facts().enums().by_value(value) {
            if !knows_enum(file, member) {
                continue;
            }
            let text = format!("`{}.{}`", member.enum_name, member.member);
            if !members.contains(&text) {
                members.push(text);
            }
        }
        if members.is_empty() {
            return;
        }
        cx.report(span)
            .arg("raw", span.source_text(file.text))
            .arg("member", members.join(" or "));
    }
}

/// The enum is declared in this file or imported under its own name. An enum the file does not
/// know makes most matches a mere common word (`'error'`, `'update'`); an import under another
/// name or of a namespace is not followed.
fn knows_enum(file: &File<'_>, member: &EnumMember) -> bool {
    let scoping = file.semantic.scoping();
    let Some(symbol) = scoping.get_root_binding(member.enum_name.as_str().into()) else {
        return false;
    };
    // By the flags, not the first declaration: a merged `namespace Source` may come first.
    if scoping.symbol_flags(symbol).is_enum() {
        // Another file may declare an enum of the same name with other members.
        return member.file == file.path;
    }
    let declaration = scoping.symbol_declaration(symbol);
    match file.semantic.nodes().kind(declaration) {
        AstKind::ImportSpecifier(specifier) => {
            specifier.imported.name().as_str() == specifier.local.name.as_str()
        }
        AstKind::ImportDefaultSpecifier(_) => true,
        _ => false,
    }
}

/// A literal under `parent` that is not a value: a type (`source: 'DASHBOARD'` in an
/// interface), an enum member's own value, a module or export name, a key (`obj['x']` and
/// `'x' in obj` too), or the text of a tagged template, which the tag reads its own way
/// (`sql`status``).
fn is_value(parent: AstKind<'_>, span: Span) -> bool {
    let key = match parent {
        AstKind::TSLiteralType(_)
        | AstKind::TSEnumMember(_)
        | AstKind::ImportDeclaration(_)
        | AstKind::ExportFromDeclaration(_)
        | AstKind::ExportAllDeclaration(_)
        | AstKind::ImportExpression(_)
        | AstKind::TSImportType(_)
        | AstKind::TSExternalModuleReference(_)
        | AstKind::TSExternalModuleDeclaration(_)
        | AstKind::ImportAttribute(_)
        | AstKind::ImportSpecifier(_)
        | AstKind::ExportSpecifier(_)
        | AstKind::Directive(_)
        | AstKind::TaggedTemplateExpression(_) => return false,
        AstKind::ObjectProperty(property) => property.key.span(),
        AstKind::PropertyDefinition(property) => property.key.span(),
        AstKind::MethodDefinition(method) => method.key.span(),
        AstKind::AccessorProperty(property) => property.key.span(),
        AstKind::TSPropertySignature(property) => property.key.span(),
        AstKind::TSMethodSignature(method) => method.key.span(),
        AstKind::BindingProperty(property) => property.key.span(),
        AstKind::AssignmentTargetPropertyProperty(property) => property.name.span(),
        AstKind::ComputedMemberExpression(member) => member.expression.span(),
        AstKind::BinaryExpression(binary) if binary.operator == BinaryOperator::In => {
            binary.left.span()
        }
        _ => return true,
    };
    key != span
}
