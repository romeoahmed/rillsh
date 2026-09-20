//! Structural checks happen for the entire source, including unused functions.
use crate::{
    Diagnostic,
    ast::{ExprKind, Literal, Module, Pattern, Rest, Statement, Unary},
};
use std::collections::HashSet;

pub fn validate(module: &Module) -> Result<(), Vec<Diagnostic>> {
    let mut errors = Vec::new();
    let mut report = |span: std::ops::Range<usize>, message: String| {
        errors.push(Diagnostic {
            span,
            message,
            incomplete: false,
        });
    };
    if let Err(message) = statements(&module.statements, true) {
        report(0..module.source.len(), message);
    }
    let negative: HashSet<_> = module
        .nodes
        .iter()
        .filter_map(|e| {
            if let ExprKind::Unary(Unary::Negate, id) = e.kind {
                Some(id.0)
            } else {
                None
            }
        })
        .collect();
    let mut depths = Vec::with_capacity(module.nodes.len());
    for (index, expr) in module.nodes.iter().enumerate() {
        let mut depth = 1;
        expr.kind.children(&mut |child| {
            depth = depth.max(depths[child.0] + 1);
        });
        depths.push(depth);
        if depth > 256 {
            report(
                expr.span.clone(),
                "syntax nesting exceeds 256 levels".into(),
            );
            break;
        }

        let result = match &expr.kind {
            ExprKind::Literal(literal) => validate_literal(literal, negative.contains(&index)),
            ExprKind::Record(fields) => unique(fields.iter().map(|(key, _)| key.as_str())),
            ExprKind::Block(block) => statements(block, false),
            ExprKind::Closure { parameters, .. } => parameters.iter().try_for_each(pattern),
            ExprKind::Match { arms, .. } => arms.iter().try_for_each(|arm| pattern(&arm.pattern)),
            _ => Ok(()),
        };
        if let Err(message) = result {
            report(expr.span.clone(), message);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn unique<'a>(names: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    let mut seen = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            return Err(format!("duplicate name '{name}'"));
        }
    }
    Ok(())
}

fn pattern(pattern: &Pattern) -> Result<(), String> {
    let mut names = Vec::new();
    pattern_names(pattern, &mut names)?;
    unique(names)
}

fn pattern_names<'a>(pattern: &'a Pattern, names: &mut Vec<&'a str>) -> Result<(), String> {
    match pattern {
        Pattern::Bind(name) => names.push(name),
        Pattern::Literal(literal) => validate_literal(literal, false)?,
        Pattern::List(items, rest) => {
            for item in items {
                pattern_names(item, names)?;
            }
            if let Some(name) = rest
                && name != "_"
            {
                names.push(name);
            }
        }
        Pattern::Record(fields, rest) => {
            unique(fields.iter().map(|(key, _)| key.as_str()))?;
            for (_, value) in fields {
                pattern_names(value, names)?;
            }
            if let Rest::Bind(name) = rest
                && name != "_"
            {
                names.push(name);
            }
        }
        Pattern::Constructor(_, Some(payload)) => pattern_names(payload, names)?,
        Pattern::Ignore | Pattern::Constructor(_, None) => {}
    }
    Ok(())
}

fn statements(block: &[Statement], top: bool) -> Result<(), String> {
    let mut names = Vec::new();
    let mut exported = false;
    for statement in block {
        match statement {
            Statement::Let(p, _) => {
                pattern_names(p, &mut names)?;
            }
            Statement::Functions(functions) => {
                for f in functions {
                    names.push(f.name.as_str());
                    f.parameters.iter().try_for_each(pattern)?;
                }
            }
            Statement::Struct(name, fields) => {
                names.push(name);
                unique(fields.iter().map(String::as_str))?;
            }
            Statement::Enum(name, cases) => {
                names.push(name);
                unique(cases.iter().map(|(name, _)| name.as_str()))?;
                for (_, fields) in cases {
                    unique(fields.iter().map(String::as_str))?;
                }
            }
            Statement::Import { name, .. } => names.push(name),
            Statement::Export(fields) => {
                if exported {
                    return Err("only one export table is permitted".into());
                }
                exported = true;
                unique(fields.iter().map(|(name, _)| name.as_str()))?;
            }
            Statement::Command(_) | Statement::Expression(_) => {}
        }
        if !top
            && matches!(
                statement,
                Statement::Struct(..)
                    | Statement::Enum(..)
                    | Statement::Import { .. }
                    | Statement::Export(_)
            )
        {
            return Err("imports, exports, and nominal declarations require top level".into());
        }
    }
    if names.contains(&"_") {
        return Err("'_' cannot name a declaration".into());
    }
    unique(names)
}

fn validate_literal(literal: &Literal, negated: bool) -> Result<(), String> {
    match literal {
        Literal::Integer(text) => {
            let text = text.replace('_', "");
            if text.parse::<i64>().is_err() && !(negated && text == "9223372036854775808") {
                return Err("integer literal is outside signed 64-bit range".into());
            }
        }
        Literal::Float(text)
            if !text
                .replace('_', "")
                .parse::<f64>()
                .is_ok_and(f64::is_finite) =>
        {
            return Err("floating literal must be finite".into());
        }
        _ => {}
    }
    Ok(())
}
