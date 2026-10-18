mod add_import;
mod branch;
mod callback;
mod callee;
mod case;
mod class_file;
mod cx;
mod def;
mod exported;
mod facts;
mod file;
mod import_block;
mod import_kind;
mod lines;
mod member;
mod own_type;
mod rename;
mod rule;
mod safety;
mod ts_type;

pub use add_import::add_import;
pub use branch::if_runs;
pub use callback::{is_callback, is_di_constructor, is_public, is_signature_given};
pub use callee::{call_of, local_callee, returns_promise, this_class};
pub use case::{Case, words};
pub use class_file::{
    TopBinding, TopClass, TopKind, class_file_classes, start_after_decorators, top_bindings,
    top_classes,
};
pub use cx::{Cx, Edit, Report, Reported};
pub use def::{
    Build, Files, FindingKind, GroupDef, Impl, RuleDef, RuleKind, RuleMeta, Ruleset, Source,
};
pub use exported::{is_exported, is_exported_node};
pub use facts::{EnumIndex, EnumMember, Fact, Facts, Package, StaticFacts};
pub use file::{File, Role, RoleDef, Roles, path_span, suffix_of};
pub use import_block::{import_blocks, import_group};
pub use import_kind::{ImportKind, import_kind, module_source, package_name};
pub use lines::{
    LineIndex, Position, has_blank_line, line_start, line_tail, one_line, opens_region,
};
pub use member::{COLLECTIONS, MemberUse, is_log_call, member_use, member_write, mutator_call};
pub use own_type::is_own_type;
pub use rename::{
    PRIVATE_MEMBER_NODES, is_ambient, is_jsx_tag, is_own_name, is_script_global, is_similar_name,
    private_member, related_symbols, rename,
};
pub use rule::{DynRule, Rule, build};
pub use safety::has_safety_comment;
pub use serde;
pub use ts_type::{
    expression_assertion, global_references, is_boolean_parameter, is_boolean_type,
    is_const_assertion, is_global, is_known_value, is_literal_value, is_nullable, is_open_key,
    is_option_key, keeps_primitive, lib_type, narrows_to_literal, outer_assertion,
    relies_on_wide_type, type_assertion, unwrapped_type,
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
        COLLECTIONS, Case, Cx, Edit, Fact, File, ImportKind, MemberUse, PRIVATE_MEMBER_NODES, Role,
        Rule, TopBinding, TopClass, TopKind, add_import, call_of, class_file_classes,
        expression_assertion, global_references, has_blank_line, has_safety_comment, if_runs,
        import_blocks, import_group, import_kind, is_ambient, is_boolean_parameter,
        is_boolean_type, is_callback, is_const_assertion, is_di_constructor, is_exported,
        is_exported_node, is_global, is_jsx_tag, is_known_value, is_literal_value, is_log_call,
        is_nullable, is_open_key, is_option_key, is_own_name, is_own_type, is_public,
        is_script_global, is_signature_given, is_similar_name, keeps_primitive, lib_type,
        line_start, line_tail, local_callee, member_use, member_write, module_source, mutator_call,
        narrows_to_literal, one_line, opens_region, outer_assertion, package_name, path_span,
        private_member, related_symbols, relies_on_wide_type, rename, returns_promise,
        start_after_decorators, this_class, top_bindings, top_classes, type_assertion,
        unwrapped_type, words,
    };
    pub use oxc_allocator::Allocator;
    pub use oxc_ast::{AstKind, AstType};
    pub use oxc_parser::Parser;
    pub use oxc_semantic::AstNode;
    pub use oxc_span::{GetSpan, Span};
    pub use serde::Deserialize;
}
