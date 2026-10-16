use oxc_ast::AstKind;
use oxc_ast::ast::{Expression, Statement};

/// The branch tests of each run of adjacent `if` statements in a statement list (`Program`,
/// `FunctionBody`, `BlockStatement`, `StaticBlock`, `SwitchCase`, `TSModuleBlock`), with their
/// `else if`s. Any other statement ends a run; another node has no runs.
pub fn if_runs<'a>(node: AstKind<'a>) -> impl Iterator<Item = Vec<&'a Expression<'a>>> {
    let statements: &'a [Statement<'a>] = match node {
        AstKind::Program(program) => &program.body,
        AstKind::FunctionBody(body) => &body.statements,
        AstKind::BlockStatement(block) => &block.body,
        AstKind::StaticBlock(block) => &block.body,
        AstKind::SwitchCase(case) => &case.consequent,
        AstKind::TSModuleBlock(block) => &block.body,
        _ => &[],
    };
    statements
        .split(|statement| !matches!(statement, Statement::IfStatement(_)))
        .filter(|run| !run.is_empty())
        .map(|run| {
            let mut tests = Vec::new();
            for statement in run {
                let mut branch = Some(statement);
                while let Some(Statement::IfStatement(statement)) = branch {
                    tests.push(&statement.test);
                    branch = statement.alternate.as_ref();
                }
            }
            tests
        })
}
