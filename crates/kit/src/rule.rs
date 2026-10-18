use oxc_ast::AstType;
use oxc_semantic::AstNode;
use serde::de::DeserializeOwned;

use crate::{Cx, Fact};

/// Write a rule without options as `struct Check {}`, not `struct Check;`: `build` reads
/// empty options as an empty table, which a unit struct rejects. Without
/// `deny_unknown_fields` a mistyped option is silently ignored.
pub trait Rule: DeserializeOwned + Send + Sync + 'static {
    const NODES: &'static [AstType] = &[];
    const USES: &'static [Fact] = &[];
    fn on_node<'a>(&self, _node: &AstNode<'a>, _cx: &mut Cx<'_, 'a>) {}
    fn on_file<'a>(&self, _cx: &mut Cx<'_, 'a>) {}
}

/// Object-safe face of [`Rule`], so the engine can hold rules of different types in one list.
pub trait DynRule: Send + Sync {
    fn uses(&self) -> &'static [Fact];
    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>);
    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>);
}

impl<R: Rule> DynRule for R {
    fn uses(&self) -> &'static [Fact] {
        R::USES
    }

    fn on_node<'a>(&self, node: &AstNode<'a>, cx: &mut Cx<'_, 'a>) {
        Rule::on_node(self, node, cx);
    }

    fn on_file<'a>(&self, cx: &mut Cx<'_, 'a>) {
        Rule::on_file(self, cx);
    }
}

/// Empty `options` text reads as an empty table.
pub fn build<R: Rule>(options: &str) -> Result<Box<dyn DynRule>, toml::de::Error> {
    let rule: R = toml::from_str(options)?;
    Ok(Box::new(rule))
}
