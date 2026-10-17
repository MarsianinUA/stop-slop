use kit::oxc::ast::ast::{
    Argument, BinaryOperator, CallExpression, ChainElement, Expression, MemberExpression,
    ObjectPropertyKind,
};
use kit::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A word of the receiver's name that makes it a logger, as in `no-empty-catch`.
    loggers: Vec<String>,
    /// A logger's method that writes a line.
    levels: Vec<String>,
    /// The last words of a name that holds a secret: `token` for `accessToken`, `apiKey` for
    /// `x-api-key`; `tokenAddress` ends with another word.
    names: Vec<String>,
}

impl Rule for Check {
    const NODES: &'static [AstType] = &[AstType::CallExpression];

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        let file = cx.file();
        let AstKind::CallExpression(call) = node.kind() else {
            return;
        };
        if !is_log_call(call, &self.loggers, &self.levels) {
            return;
        }
        let mut secrets = Vec::new();
        for argument in call.arguments.iter().filter_map(Argument::as_expression) {
            self.secrets(file, argument, &mut secrets);
        }
        for span in secrets {
            // One line in the text output, also for an object key over several lines.
            let value = span
                .source_text(file.text)
                .split_whitespace()
                .collect::<Vec<_>>();
            cx.report(span).arg("value", value.join(" "));
        }
    }
}

impl Check {
    /// Where a logged value puts a secret into the line as it is: a read named like one (`token`,
    /// `req.headers.authorization`), an object key named like one over a read (`{ token }`,
    /// `{ apiKey: config.key }`), also inside a template, `+`, `??`, `?:`, an object (and its
    /// spread), `String(..)`, `JSON.stringify(..)` and `.toString()`. Another call logs what it
    /// returns (`mask(token)`, `token.slice(-4)`), and a member logs itself (`token.length`).
    fn secrets(&self, file: &File<'_>, expression: &Expression<'_>, found: &mut Vec<Span>) {
        match expression.get_inner_expression() {
            Expression::TemplateLiteral(template) => {
                for part in &template.expressions {
                    self.secrets(file, part, found);
                }
            }
            Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
                self.secrets(file, &binary.left, found);
                self.secrets(file, &binary.right, found);
            }
            Expression::LogicalExpression(logical) => {
                self.secrets(file, &logical.left, found);
                self.secrets(file, &logical.right, found);
            }
            Expression::ConditionalExpression(conditional) => {
                self.secrets(file, &conditional.consequent, found);
                self.secrets(file, &conditional.alternate, found);
            }
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::SpreadProperty(spread) => {
                            self.secrets(file, &spread.argument, found);
                        }
                        ObjectPropertyKind::ObjectProperty(property) => {
                            let named = property
                                .key
                                .static_name()
                                .is_some_and(|key| self.is_secret(&key));
                            if named && carries(file, &property.value) {
                                found.push(property.span);
                            } else {
                                self.secrets(file, &property.value, found);
                            }
                        }
                    }
                }
            }
            Expression::CallExpression(call) => {
                if let Some(value) = passed_on(file, call) {
                    self.secrets(file, value, found);
                }
            }
            Expression::ChainExpression(chain) => match &chain.expression {
                ChainElement::CallExpression(call) => {
                    if let Some(value) = passed_on(file, call) {
                        self.secrets(file, value, found);
                    }
                }
                element => {
                    if element
                        .as_member_expression()
                        .and_then(read_name)
                        .is_some_and(|name| self.is_secret(name))
                    {
                        found.push(chain.span);
                    }
                }
            },
            Expression::Identifier(id) => {
                if self.is_secret(id.name.as_str()) {
                    found.push(id.span);
                }
            }
            expression => {
                if expression
                    .as_member_expression()
                    .and_then(read_name)
                    .is_some_and(|name| self.is_secret(name))
                {
                    found.push(expression.span());
                }
            }
        }
    }

    /// The name ends with the words of a secret name, in any case: `refreshToken`, `API_KEY`,
    /// `x-api-key`, `Authorization`.
    fn is_secret(&self, name: &str) -> bool {
        let name = words(name);
        self.names.iter().any(|secret| {
            let secret = words(secret);
            name.len() >= secret.len()
                && name[name.len() - secret.len()..]
                    .iter()
                    .zip(&secret)
                    .all(|(word, part)| word.eq_ignore_ascii_case(part))
        })
    }
}

/// `o.token`, `o['token']`, `o.#token`: the name the member reads.
fn read_name<'a>(member: &'a MemberExpression<'a>) -> Option<&'a str> {
    match member {
        MemberExpression::PrivateFieldExpression(private) => Some(private.field.name.as_str()),
        member => member.static_property_name(),
    }
}

/// The value a call gives back whole: of global `String(x)` and `JSON.stringify(x)`, of
/// `x.toString()`.
fn passed_on<'a>(file: &File<'_>, call: &'a CallExpression<'a>) -> Option<&'a Expression<'a>> {
    let first = || call.arguments.first().and_then(Argument::as_expression);
    match call.callee.get_inner_expression() {
        callee if is_global(file, callee, "String") => first(),
        Expression::StaticMemberExpression(member)
            if member.property.name == "stringify" && is_global(file, &member.object, "JSON") =>
        {
            first()
        }
        Expression::StaticMemberExpression(member)
            if member.property.name == "toString" && call.arguments.is_empty() =>
        {
            Some(&member.object)
        }
        _ => None,
    }
}

/// The value puts a read into the line: a read, or a template, `+`, `??`, `?:` or pass-on call
/// with one in it. A literal, another call, `!x` or a function does not.
fn carries(file: &File<'_>, value: &Expression<'_>) -> bool {
    match value.get_inner_expression() {
        Expression::Identifier(_) => true,
        Expression::TemplateLiteral(template) => {
            template.expressions.iter().any(|part| carries(file, part))
        }
        Expression::BinaryExpression(binary) => {
            binary.operator == BinaryOperator::Addition
                && (carries(file, &binary.left) || carries(file, &binary.right))
        }
        Expression::LogicalExpression(logical) => {
            carries(file, &logical.left) || carries(file, &logical.right)
        }
        Expression::ConditionalExpression(conditional) => {
            carries(file, &conditional.consequent) || carries(file, &conditional.alternate)
        }
        Expression::CallExpression(call) => {
            passed_on(file, call).is_some_and(|value| carries(file, value))
        }
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::CallExpression(call) => {
                passed_on(file, call).is_some_and(|value| carries(file, value))
            }
            _ => true,
        },
        value => value.is_member_expression(),
    }
}
