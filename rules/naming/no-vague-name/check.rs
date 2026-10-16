use kit::oxc::ast::ast::Expression;
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// Red words for the first or the last word of a type or function name, without case.
    words: Vec<String>,
    /// Whole variable names.
    names: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::MethodDefinition];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let Some((name, span)) = private_member(cx.file(), node) else {
            return;
        };
        if let Some(word) = self.red_word(name) {
            cx.report(span).arg("name", name).arg("word", word);
        }
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let scoping = file.semantic.scoping();
        let nodes = file.semantic.nodes();
        for symbol in scoping.symbol_ids() {
            let name = scoping.symbol_name(symbol);
            let declaration = scoping.symbol_declaration(symbol);
            // The own name of a function or class expression repeats the variable it
            // initializes.
            let word = match nodes.kind(declaration) {
                AstKind::Class(class) if class.is_declaration() => self.red_word(name),
                AstKind::Function(function) if function.is_declaration() => self.red_word(name),
                AstKind::TSInterfaceDeclaration(_)
                | AstKind::TSTypeAliasDeclaration(_)
                | AstKind::TSEnumDeclaration(_) => self.red_word(name),
                AstKind::VariableDeclarator(declarator)
                    if declarator.id.is_binding_identifier() =>
                {
                    let names_function = declarator.init.as_ref().is_some_and(|init| {
                        matches!(
                            init.get_inner_expression(),
                            Expression::FunctionExpression(_)
                                | Expression::ArrowFunctionExpression(_)
                                | Expression::ClassExpression(_)
                        )
                    });
                    if names_function {
                        self.red_word(name)
                    } else {
                        // A `for..of` or `for..in` variable is an element of the collection in
                        // the loop head, like a callback parameter, where `item` is the advice.
                        let is_loop_variable = matches!(
                            nodes.parent_kind(nodes.parent_id(declaration)),
                            AstKind::ForOfStatement(_) | AstKind::ForInStatement(_)
                        );
                        (!is_loop_variable && self.names.iter().any(|vague| vague == name))
                            .then_some(name)
                    }
                }
                _ => None,
            };
            let Some(word) = word else {
                continue;
            };
            if is_own_name(file, symbol) {
                cx.report(scoping.symbol_span(symbol))
                    .arg("name", name)
                    .arg("word", word);
            }
        }
    }
}

impl Check {
    /// The red word that starts or ends the name, also with a plural `s` (`Utils`): in the
    /// middle it names another thing (`CacheManagerConfig` is a config).
    fn red_word(&self, name: &str) -> Option<&str> {
        let words = words(name);
        [words.first(), words.last()]
            .into_iter()
            .flatten()
            .find_map(|word| {
                let singular = word.strip_suffix(['s', 'S']).unwrap_or(word);
                self.words.iter().find(|red| {
                    red.eq_ignore_ascii_case(word) || red.eq_ignore_ascii_case(singular)
                })
            })
            .map(String::as_str)
    }
}
