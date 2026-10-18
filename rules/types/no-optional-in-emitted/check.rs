use kit::oxc::semantic::NodeId;
use kit::prelude::*;

/// Words of the file name and the owner's name that mark a shape as sent out, and words that
/// mark it as an input: an input word wins (`CreateOrderDto`).
#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    emitted: Vec<String>,
    input: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[
        AstType::TSPropertySignature,
        AstType::PropertyDefinition,
        AstType::FormalParameter,
    ];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let nodes = file.semantic.nodes();
        let (key, annotation, inside) = match node.kind() {
            AstKind::TSPropertySignature(field) if field.optional => {
                (field.key.span(), &field.type_annotation, node.id())
            }
            AstKind::PropertyDefinition(field) if field.optional => {
                (field.key.span(), &field.type_annotation, node.id())
            }
            // `constructor(public note?: string)` declares a field of the constructor's class.
            AstKind::FormalParameter(parameter)
                if parameter.optional && parameter.has_modifier() =>
            {
                let function = nodes.parent_id(nodes.parent_id(node.id()));
                (
                    parameter.pattern.span(),
                    &parameter.type_annotation,
                    function,
                )
            }
            _ => return,
        };
        let Some(owner) = owner(file, inside) else {
            return;
        };
        let name = owner_name(file, owner);
        let mut words = file_words(file);
        words.extend(name_words(name));
        let has = |list: &[String]| words.iter().any(|word| list.contains(word));
        // The export check scans the owner's references, so it runs last.
        if !has(&self.emitted) || has(&self.input) || !is_exported_node(file, owner) {
            return;
        }
        let end = annotation
            .as_ref()
            .map_or(key.end, |annotation| annotation.span.end);
        cx.report(Span::new(key.start, end)).arg("owner", name);
    }
}

/// The nearest interface, type alias or class above `inside`. A field in a parameter or
/// signature type (`options?: { verbose?: boolean }`) has none: it is an input.
fn owner<'s, 'a>(file: &'s File<'a>, inside: NodeId) -> Option<&'s AstNode<'a>> {
    for ancestor in file.semantic.nodes().ancestors(inside) {
        match ancestor.kind() {
            AstKind::TSInterfaceDeclaration(_)
            | AstKind::TSTypeAliasDeclaration(_)
            | AstKind::Class(_) => return Some(ancestor),
            AstKind::FormalParameter(_)
            | AstKind::FormalParameterRest(_)
            | AstKind::Function(_)
            | AstKind::ArrowFunctionExpression(_)
            | AstKind::TSMethodSignature(_) => return None,
            _ => {}
        }
    }
    None
}

/// The owner's own name; for a class expression the variable it initializes, for an unnamed
/// `export default class` `default`.
fn owner_name<'a>(file: &File<'a>, owner: &AstNode<'a>) -> &'a str {
    let span = match owner.kind() {
        AstKind::TSInterfaceDeclaration(interface) => Some(interface.id.span),
        AstKind::TSTypeAliasDeclaration(alias) => Some(alias.id.span),
        AstKind::Class(class) => match file.semantic.nodes().parent_kind(owner.id()) {
            AstKind::VariableDeclarator(declarator) => Some(declarator.id.span()),
            _ => class.id.as_ref().map(|id| id.span),
        },
        _ => None,
    };
    span.map_or("default", |span| span.source_text(file.text))
}

/// `account-history.response.dto.ts` -> account, history, response, dto.
fn file_words(file: &File<'_>) -> Vec<String> {
    file.path
        .file_stem()
        .unwrap_or_default()
        .split(['.', '-', '_'])
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// PascalCase words in lower case, an abbreviation as one word:
/// `ListAccountHistoryQueryDto` -> list, account, history, query, dto; `UserDTO` -> user, dto.
fn name_words(name: &str) -> Vec<String> {
    let chars: Vec<char> = name.chars().collect();
    let mut words = Vec::new();
    let mut word = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let starts_word = c.is_uppercase()
            && i > 0
            && (!chars[i - 1].is_uppercase()
                || chars.get(i + 1).is_some_and(|next| next.is_lowercase()));
        if starts_word && !word.is_empty() {
            words.push(std::mem::take(&mut word));
        }
        word.extend(c.to_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    words
}
