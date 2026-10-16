mod callback;
mod case;
mod class_file;
mod cx;
mod def;
mod exported;
mod facts;
mod file;
mod import_kind;
mod lines;
mod member;
mod own_type;
mod rename;
mod rule;
mod safety;
mod ts_type;

pub use callback::is_callback;
pub use case::{Case, words};
pub use class_file::{
    TopBinding, TopClass, TopKind, start_after_decorators, top_bindings, top_classes,
};
pub use cx::{Cx, Edit, Report, Reported};
pub use def::{
    Build, Files, FindingKind, GroupDef, Impl, RuleDef, RuleKind, RuleMeta, Ruleset, Source,
};
pub use exported::{is_exported, is_exported_node};
pub use facts::{EnumIndex, EnumMember, Fact, Facts, Package, StaticFacts};
pub use file::{File, Role, RoleDef, Roles, path_span, suffix_of};
pub use import_kind::{ImportKind, import_kind};
pub use lines::{LineIndex, Position};
pub use member::{MemberUse, member_use};
pub use own_type::is_own_type;
pub use rename::{
    PRIVATE_MEMBER_NODES, is_ambient, is_jsx_tag, is_own_name, is_script_global, is_similar_name,
    private_member, related_symbols, rename,
};
pub use rule::{DynRule, Rule, build};
pub use safety::has_safety_comment;
pub use serde;
pub use ts_type::{
    expression_assertion, is_const_assertion, is_global, is_known_value, is_nullable, is_open_key,
    lib_type, outer_assertion, type_assertion, unwrapped_type,
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
        Case, Cx, Edit, Fact, File, ImportKind, MemberUse, PRIVATE_MEMBER_NODES, Role, Rule,
        TopBinding, TopClass, TopKind, expression_assertion, has_safety_comment, import_kind,
        is_ambient, is_callback, is_const_assertion, is_exported, is_exported_node, is_global,
        is_jsx_tag, is_known_value, is_nullable, is_open_key, is_own_name, is_own_type,
        is_script_global, is_similar_name, lib_type, member_use, outer_assertion, path_span,
        private_member, related_symbols, rename, start_after_decorators, top_bindings, top_classes,
        type_assertion, unwrapped_type, words,
    };
    pub use oxc_allocator::Allocator;
    pub use oxc_ast::{AstKind, AstType};
    pub use oxc_parser::Parser;
    pub use oxc_semantic::AstNode;
    pub use oxc_span::{GetSpan, Span};
    pub use serde::Deserialize;
}
