mod case;
mod class_file;
mod cx;
mod def;
mod exported;
mod facts;
mod file;
mod import_kind;
mod lines;
mod own_type;
mod rename;
mod rule;
mod safety;
mod ts_type;

pub use case::{Case, words};
pub use class_file::{TopBinding, TopClass, TopKind, top_bindings, top_classes};
pub use cx::{Cx, Edit, Report, Reported};
pub use def::{
    Build, Files, FindingKind, GroupDef, Impl, RuleDef, RuleKind, RuleMeta, Ruleset, Source,
};
pub use exported::{is_exported, is_exported_node};
pub use facts::{EnumIndex, EnumMember, Fact, Facts, Package, StaticFacts};
pub use file::{File, Role, RoleDef, Roles, suffix_of};
pub use import_kind::{ImportKind, import_kind};
pub use lines::{LineIndex, Position};
pub use own_type::is_own_type;
pub use rename::{is_own_name, private_member, rename};
pub use rule::{DynRule, Rule, build};
pub use safety::has_safety_comment;
pub use serde;
pub use ts_type::{
    expression_assertion, is_const_assertion, is_known_value, is_open_key, lib_type,
    type_assertion, unwrapped_type,
};

pub mod oxc {
    pub use oxc_allocator as allocator;
    pub use oxc_ast as ast;
    pub use oxc_parser as parser;
    pub use oxc_semantic as semantic;
    pub use oxc_span as span;
}

pub mod prelude {
    pub use crate::{
        Case, Cx, Edit, Fact, File, ImportKind, Role, Rule, TopBinding, TopClass, TopKind,
        expression_assertion, has_safety_comment, import_kind, is_const_assertion, is_exported,
        is_exported_node, is_known_value, is_open_key, is_own_name, is_own_type, lib_type,
        private_member, rename, top_bindings, top_classes, type_assertion, unwrapped_type, words,
    };
    pub use oxc_allocator::Allocator;
    pub use oxc_ast::{AstKind, AstType};
    pub use oxc_parser::Parser;
    pub use oxc_semantic::AstNode;
    pub use oxc_span::{GetSpan, Span};
    pub use serde::Deserialize;
}
