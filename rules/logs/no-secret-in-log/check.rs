use kit::oxc::ast::ast::{
    Argument, BinaryOperator, ChainElement, Expression, MemberExpression, ObjectPropertyKind,
};
use kit::prelude::*;
use kit::serde::Deserializer;

#[derive(Deserialize)]
#[serde(crate = "kit::serde", deny_unknown_fields)]
pub struct Check {
    /// A word of the receiver's name that makes it a logger, as in `no-empty-catch`.
    loggers: Vec<String>,
    /// A logger's method that writes a line.
    levels: Vec<String>,
    /// The last words of a name that holds a secret: `token` for `accessToken`, `apiKey` for
    /// `x-api-key`; `tokenAddress` ends with another word.
    #[serde(deserialize_with = "split_names")]
    names: Vec<Vec<String>>,
}

fn split_names<'de, D: Deserializer<'de>>(names: D) -> Result<Vec<Vec<String>>, D::Error> {
    let names = Vec::<String>::deserialize(names)?;
    Ok(names
        .iter()
        .map(|name| words(name).into_iter().map(String::from).collect())
        .collect())
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
    /// `{ apiKey: config.key }`), in any of its `parts` and inside an object (and its spread).
    /// Another call logs what it returns (`mask(token)`, `token.slice(-4)`), and a member logs
    /// itself (`token.length`).
    fn secrets(&self, file: &File<'_>, value: &Expression<'_>, found: &mut Vec<Span>) {
        parts(file, value, &mut |part| match part {
            Expression::ObjectExpression(object) => {
                for property in &object.properties {
                    match property {
                        ObjectPropertyKind::SpreadProperty(spread) => {
                            self.secrets(file, &spread.argument, found);
                        }
                        ObjectPropertyKind::ObjectProperty(property) => {
                            let leaks = carries(file, &property.value)
                                && property
                                    .key
                                    .static_name()
                                    .is_some_and(|key| self.is_secret(&key));
                            if leaks {
                                found.push(property.span);
                            } else {
                                self.secrets(file, &property.value, found);
                            }
                        }
                    }
                }
            }
            Expression::Identifier(id) => {
                if self.is_secret(id.name.as_str()) {
                    found.push(id.span);
                }
            }
            part => {
                if read_name(part).is_some_and(|name| self.is_secret(name)) {
                    found.push(part.span());
                }
            }
        });
    }

    /// The name ends with the words of a secret name, in any case: `refreshToken`, `API_KEY`,
    /// `x-api-key`, `Authorization`.
    fn is_secret(&self, name: &str) -> bool {
        let name = words(name);
        self.names.iter().any(|secret| {
            name.len() >= secret.len()
                && name[name.len() - secret.len()..]
                    .iter()
                    .zip(secret)
                    .all(|(word, part)| word.eq_ignore_ascii_case(part))
        })
    }
}

/// The parts of a value that go into the line as they are: through a template, `+`, `??`, `?:`,
/// `String(..)`, `JSON.stringify(..)` and `.toString()`.
fn parts<'a, F: FnMut(&'a Expression<'a>)>(
    file: &File<'_>,
    value: &'a Expression<'a>,
    each: &mut F,
) {
    match value.get_inner_expression() {
        Expression::TemplateLiteral(template) => {
            for part in &template.expressions {
                parts(file, part, each);
            }
        }
        Expression::BinaryExpression(binary) if binary.operator == BinaryOperator::Addition => {
            parts(file, &binary.left, each);
            parts(file, &binary.right, each);
        }
        Expression::LogicalExpression(logical) => {
            parts(file, &logical.left, each);
            parts(file, &logical.right, each);
        }
        Expression::ConditionalExpression(conditional) => {
            parts(file, &conditional.consequent, each);
            parts(file, &conditional.alternate, each);
        }
        value => match passed_on(file, value) {
            Some(value) => parts(file, value, each),
            None => each(value),
        },
    }
}

/// `o.token`, `o['token']`, `o.#token`, `o?.token`: the name the member reads.
fn read_name<'a>(value: &'a Expression<'a>) -> Option<&'a str> {
    let member = match value {
        Expression::ChainExpression(chain) => chain.expression.as_member_expression(),
        value => value.as_member_expression(),
    }?;
    match member {
        MemberExpression::PrivateFieldExpression(private) => Some(private.field.name.as_str()),
        member => member.static_property_name(),
    }
}

/// The value a call gives back whole, also through `?.`: of global `String(x)` and
/// `JSON.stringify(x)`, of `x.toString()`.
fn passed_on<'a>(file: &File<'_>, value: &'a Expression<'a>) -> Option<&'a Expression<'a>> {
    let call = match value {
        Expression::CallExpression(call) => call,
        Expression::ChainExpression(chain) => match &chain.expression {
            ChainElement::CallExpression(call) => call,
            _ => return None,
        },
        _ => return None,
    };
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

/// One of the value's `parts` is a read. A literal, `undefined`, another call, `!x` or a function
/// is not.
fn carries(file: &File<'_>, value: &Expression<'_>) -> bool {
    let mut read = false;
    parts(file, value, &mut |part| {
        read |= match part {
            Expression::Identifier(_) => !is_global(file, part, "undefined"),
            Expression::ChainExpression(chain) => {
                !matches!(chain.expression, ChainElement::CallExpression(_))
            }
            part => part.is_member_expression(),
        };
    });
    read
}
