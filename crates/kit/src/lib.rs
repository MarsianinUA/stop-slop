mod cx;
mod def;
mod facts;
mod file;
mod lines;
mod rule;

pub use cx::{Cx, Edit, Report, Reported};
pub use def::{
    Build, Files, FindingKind, GroupDef, Impl, RuleDef, RuleKind, RuleMeta, Ruleset, Source,
};
pub use facts::{EnumIndex, EnumMember, Fact, Facts, Package, StaticFacts};
pub use file::{File, Role, RoleDef, Roles, suffix_of};
pub use lines::{LineIndex, Position};
pub use rule::{DynRule, Rule, build};
pub use serde;

pub mod oxc {
    pub use oxc_allocator as allocator;
    pub use oxc_ast as ast;
    pub use oxc_parser as parser;
    pub use oxc_semantic as semantic;
    pub use oxc_span as span;
}

pub mod prelude {
    pub use crate::{Cx, Edit, Fact, File, Role, Rule};
    pub use oxc_allocator::Allocator;
    pub use oxc_ast::{AstKind, AstType};
    pub use oxc_parser::Parser;
    pub use oxc_semantic::AstNode;
    pub use oxc_span::{GetSpan, Span};
    pub use serde::Deserialize;
}
