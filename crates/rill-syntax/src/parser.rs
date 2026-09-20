//! Grammar composition over contextual Logos tokens, with whole-entry validation.

use crate::{
    ast::{
        Argument, Arm, Binary, CommandPart, Expr, ExprId, ExprKind, Function, Literal, Module,
        Pattern, Rest, Stage, Statement, Unary,
    },
    token::{self, Kind, Span, Token},
};
use chumsky::{
    input::{Input, MappedInput},
    prelude::*,
};

/// Source diagnostic suitable for either a script or an interactive entry.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct Diagnostic {
    pub span: Span,
    pub message: String,
    pub incomplete: bool,
}

type State = chumsky::inspector::TruncateState<Expr>;
type Extra<'a> = extra::Full<Rich<'a, Token>, State, bool>;

fn add(state: &mut State, kind: ExprKind, span: SimpleSpan) -> ExprId {
    let id = ExprId(state.len());
    state.push(Expr {
        kind,
        span: span.into_range(),
    });
    id
}

fn node<'a>(
    kind: ExprKind,
    e: &mut chumsky::input::MapExtra<'a, '_, TokenInput<'a>, Extra<'a>>,
) -> ExprId {
    let span = e.span();
    add(e.state(), kind, span)
}

type TokenInput<'a> = MappedInput<'a, Token, SimpleSpan, &'a [(Token, SimpleSpan)]>;
type P<'a, T> = Boxed<'a, 'a, TokenInput<'a>, T, Extra<'a>>;

fn token<'a>(kind: Kind) -> P<'a, ()> {
    any()
        .filter(move |t: &Token| t.kind == kind)
        .ignored()
        .boxed()
}
fn nl<'a>() -> chumsky::combinator::Repeated<P<'a, ()>, (), TokenInput<'a>, Extra<'a>> {
    token(Kind::Newline).repeated()
}
fn soft<'a>() -> chumsky::combinator::Repeated<P<'a, ()>, (), TokenInput<'a>, Extra<'a>> {
    any()
        .try_map_with(|t: Token, e| {
            if t.kind == Kind::Newline && *e.ctx() {
                Ok(())
            } else {
                Err(Rich::custom(e.span(), "expected continuation newline"))
            }
        })
        .boxed()
        .repeated()
}
fn separator<'a>() -> P<'a, ()> {
    token(Kind::Newline)
        .or(token(Kind::Semicolon))
        .repeated()
        .at_least(1)
        .ignored()
        .boxed()
}
fn name<'a>() -> P<'a, String> {
    select! { Token { kind: Kind::Name(name), .. } => name }.boxed()
}
fn string<'a>() -> P<'a, String> {
    select! { Token { kind: Kind::String(text), .. } => text }.boxed()
}
fn field<'a>() -> P<'a, String> {
    any()
        .try_map(|t: Token, span| {
            t.kind
                .field_name()
                .map(str::to_owned)
                .ok_or_else(|| Rich::custom(span, "expected field name"))
        })
        .boxed()
}
fn key<'a>() -> P<'a, (String, bool)> {
    any()
        .try_map(|t: Token, span| match t.kind {
            Kind::String(s) => Ok((s, false)),
            Kind::Name(s) => {
                let shorthand = s != "_";
                Ok((s, shorthand))
            }
            kind => kind
                .field_name()
                .map(|s| (s.to_owned(), false))
                .ok_or_else(|| Rich::custom(span, "expected field key")),
        })
        .boxed()
}
fn literal<'a>() -> P<'a, Literal> {
    select! {
        Token { kind: Kind::Number(n), .. } => if n.contains(['.', 'e', 'E']) { Literal::Float(n) } else { Literal::Integer(n) },
        Token { kind: Kind::String(s), .. } => Literal::String(s),
        Token { kind: Kind::True, .. } => Literal::Bool(true),
        Token { kind: Kind::False, .. } => Literal::Bool(false),
        Token { kind: Kind::Null, .. } => Literal::Null,
    }.or(token(Kind::OpenParen).then(nl()).then(token(Kind::CloseParen)).to(Literal::Unit)).boxed()
}
fn path<'a>() -> P<'a, Vec<String>> {
    name()
        .then(
            token(Kind::Dot)
                .ignore_then(name())
                .repeated()
                .collect::<Vec<_>>(),
        )
        .map(|(first, rest)| std::iter::once(first).chain(rest).collect::<Vec<_>>())
        .boxed()
}
fn pattern<'a>() -> P<'a, Pattern> {
    let name = name();
    let key = key();
    let literal = literal();
    let path = path();
    recursive(|pattern| {
        let rest = token(Kind::Rest)
            .ignore_then(name.clone().or_not())
            .map(|name| name.map_or(Rest::Ignore, Rest::Bind));
        let fields = key
            .clone()
            .then(token(Kind::Colon).ignore_then(pattern.clone()).or_not())
            .try_map(|((key, shorthand), value), span| {
                let p = value
                    .or_else(|| shorthand.then(|| Pattern::Bind(key.clone())))
                    .ok_or_else(|| Rich::custom(span, "expected ':' and a pattern"))?;
                Ok((key, p))
            });
        let record = fields
            .padded_by(nl())
            .separated_by(token(Kind::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .then(rest.clone().padded_by(nl()).or_not())
            .delimited_by(token(Kind::OpenBrace).then(nl()), token(Kind::CloseBrace))
            .map(|(fields, rest)| Pattern::Record(fields, rest.unwrap_or(Rest::Exact)));
        let list = pattern
            .clone()
            .padded_by(nl())
            .separated_by(token(Kind::Comma))
            .allow_trailing()
            .collect::<Vec<_>>()
            .then(rest.clone().padded_by(nl()).or_not())
            .delimited_by(token(Kind::OpenList).then(nl()), token(Kind::CloseList))
            .map(|(items, rest)| {
                Pattern::List(
                    items,
                    rest.map(|r| match r {
                        Rest::Bind(n) => n,
                        _ => "_".into(),
                    }),
                )
            });
        let constructor =
            path.clone()
                .then(record.clone().or_not())
                .try_map(|(path, payload), span| {
                    if payload.is_none() && path.len() == 1 {
                        Err(Rich::custom(span, "expected constructor pattern"))
                    } else {
                        Ok(Pattern::Constructor(path, payload.map(Box::new)))
                    }
                });
        let negative =
            token(Kind::Minus)
                .ignore_then(literal.clone())
                .try_map(|lit, span| match lit {
                    Literal::Integer(n) => Ok(Pattern::Literal(Literal::Integer(format!("-{n}")))),
                    Literal::Float(n) => Ok(Pattern::Literal(Literal::Float(format!("-{n}")))),
                    _ => Err(Rich::custom(span, "expected number after '-'")),
                });
        choice((
            constructor,
            record,
            list,
            negative,
            literal.clone().map(Pattern::Literal),
            name.clone().map(|n| {
                if n == "_" {
                    Pattern::Ignore
                } else {
                    Pattern::Bind(n)
                }
            }),
            pattern.delimited_by(
                token(Kind::OpenParen).then(nl()),
                nl().then(token(Kind::CloseParen)),
            ),
        ))
        .boxed()
    })
    .boxed()
}
fn parameter<'a>() -> P<'a, Pattern> {
    // Parameter headers use atomic patterns: an unparenthesized name is always a binding.
    name()
        .then_ignore(token(Kind::Dot).not())
        .map(|name| {
            if name == "_" {
                Pattern::Ignore
            } else {
                Pattern::Bind(name)
            }
        })
        .or(pattern())
        .boxed()
}
fn separated_parameter<'a>() -> P<'a, Pattern> {
    any()
        .filter(|t: &Token| t.separated && t.kind != Kind::Newline)
        .rewind()
        .ignored()
        .or(nl().at_least(1).ignored())
        .ignore_then(parameter())
        .boxed()
}

fn pipeline(expression: P<'_, ExprId>) -> P<'_, Vec<Stage>> {
    let name = name();
    let argument_value = token(Kind::OpenParen)
        .then(nl())
        .ignore_then(expression)
        .then_ignore(nl())
        .then_ignore(token(Kind::CloseParen))
        .with_ctx(true)
        .or(name
            .clone()
            .map_with(|name, e| node(ExprKind::Name(name), e)));
    let argument = select! { Token { kind: Kind::Word(word) | Kind::String(word), .. } => Argument::Literal(word) }
        .or(token(Kind::Substitute).ignore_then(argument_value.clone()).map(Argument::Value))
        .or(token(Kind::Spread).ignore_then(argument_value).map(Argument::Spread));
    let scalar = argument.clone().try_map(|argument, span| {
        if matches!(argument, Argument::Spread(_)) {
            Err(Rich::custom(
                span,
                "a command name or redirection path cannot use list spread",
            ))
        } else {
            Ok(argument)
        }
    });
    let redirect = select! { Token { kind: Kind::Redirect(kind), .. } if kind != token::Redirect::ErrorToOutput => kind }
        .then(scalar.clone()).map(|(kind, value)| CommandPart::Redirect(kind, Some(value)))
        .or(token(Kind::Redirect(token::Redirect::ErrorToOutput)).to(CommandPart::Redirect(token::Redirect::ErrorToOutput, None)));
    let part = redirect.or(any()
        .filter(|t: &Token| t.separated && t.kind != Kind::Newline)
        .rewind()
        .ignore_then(argument)
        .map(CommandPart::Argument));
    let stage = token(Kind::Command)
        .ignore_then(scalar)
        .then(part.repeated().collect::<Vec<_>>())
        .map(|(executable, rest)| Stage {
            parts: std::iter::once(CommandPart::Argument(executable))
                .chain(rest)
                .collect(),
        });
    let pipeline = stage
        .separated_by(nl().then(token(Kind::Pipe)).then(nl()))
        .at_least(1)
        .collect::<Vec<_>>();
    pipeline.boxed()
}
fn closure(block: P<'_, Vec<Statement>>) -> P<'_, ExprId> {
    let name = name();
    let parameter = parameter();
    let separated_parameter = separated_parameter();
    let closure = token(Kind::OpenBrace)
        .then(nl())
        .ignore_then(parameter.clone())
        .then(separated_parameter.clone().repeated().collect::<Vec<_>>())
        .then_ignore(nl())
        .then_ignore(token(Kind::Arrow))
        .then(block.clone().with_ctx(false))
        .then_ignore(token(Kind::CloseBrace))
        .map_with(|((first, rest), statements), e| {
            let span = e.span();
            let body = add(e.state(), ExprKind::Block(statements), span);
            add(
                e.state(),
                ExprKind::Closure {
                    name: None,
                    parameters: std::iter::once(first).chain(rest).collect(),
                    body,
                },
                span,
            )
        });
    let recursive_closure = token(Kind::Rec)
        .ignore_then(token(Kind::OpenBrace))
        .then(nl())
        .ignore_then(name.clone())
        .then(
            separated_parameter
                .clone()
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(nl())
        .then_ignore(token(Kind::Arrow))
        .then(block.with_ctx(false))
        .then_ignore(token(Kind::CloseBrace))
        .map_with(|((name, parameters), statements), e| {
            let span = e.span();
            let body = add(e.state(), ExprKind::Block(statements), span);
            add(
                e.state(),
                ExprKind::Closure {
                    name: Some(name),
                    parameters,
                    body,
                },
                span,
            )
        });
    recursive_closure.or(closure).boxed()
}
fn aggregates<'a>(expression: P<'a, ExprId>, block: P<'a, Vec<Statement>>) -> P<'a, ExprId> {
    let key = key();
    let body = block
        .with_ctx(false)
        .delimited_by(token(Kind::OpenBrace), token(Kind::CloseBrace))
        .map_with(|statements, e| node(ExprKind::Block(statements), e));
    let grouped = token(Kind::OpenParen)
        .ignore_then(nl())
        .ignore_then(expression.clone())
        .then_ignore(nl())
        .then_ignore(token(Kind::CloseParen))
        .map_with(|id, e| node(ExprKind::Group(id), e))
        .with_ctx(true);
    let list = expression
        .clone()
        .padded_by(nl())
        .separated_by(token(Kind::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(token(Kind::OpenList).then(nl()), token(Kind::CloseList))
        .map_with(|items, e| node(ExprKind::List(items), e))
        .with_ctx(true);
    let record_field = key
        .clone()
        .then(
            token(Kind::Colon)
                .then(nl())
                .ignore_then(expression)
                .or_not(),
        )
        .try_map_with(|((key, shorthand), value), e| {
            let span = e.span();
            let value = value
                .or_else(|| shorthand.then(|| add(e.state(), ExprKind::Name(key.clone()), span)))
                .ok_or_else(|| Rich::custom(span, "expected ':' and a field value"))?;
            Ok((key, value))
        });
    let record = record_field
        .padded_by(nl())
        .separated_by(token(Kind::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(token(Kind::OpenBrace).then(nl()), token(Kind::CloseBrace))
        .map_with(|fields, e| node(ExprKind::Record(fields), e))
        .with_ctx(true);
    choice((record, list, grouped, token(Kind::Do).ignore_then(body))).boxed()
}
fn application<'a>(expression: P<'a, ExprId>, atom: P<'a, ExprId>) -> P<'a, ExprId> {
    let suffix = any()
        .filter(|t: &Token| !t.separated && matches!(t.kind, Kind::Dot | Kind::OpenList))
        .rewind()
        .ignore_then(
            token(Kind::Dot)
                .ignore_then(field())
                .map(Ok)
                .or(token(Kind::OpenList)
                    .then(nl())
                    .ignore_then(expression)
                    .then_ignore(nl())
                    .then_ignore(token(Kind::CloseList))
                    .map(Err)
                    .with_ctx(true)),
        );
    let suffixed = atom.foldl_with(suffix.repeated(), |base, suffix, e| {
        let end = e.span().end;
        let span = SimpleSpan::from(e.state()[base.0].span.start..end);
        add(
            e.state(),
            match suffix {
                Ok(name) => ExprKind::Field(base, name),
                Err(index) => ExprKind::Index(base, index),
            },
            span,
        )
    });
    let application = suffixed.clone().foldl_with(
        any()
            .filter(|t: &Token| t.separated && t.kind != Kind::Newline)
            .rewind()
            .ignored()
            .or(soft().at_least(1).ignored())
            .ignore_then(suffixed)
            .repeated(),
        |function, argument, e| {
            let span = e.span();
            add(e.state(), ExprKind::Call(function, argument), span)
        },
    );
    let unary = token(Kind::Minus)
        .to(Unary::Negate)
        .or(token(Kind::Not).to(Unary::Not))
        .then_ignore(nl())
        .repeated()
        .foldr_with(application, |op, value, e| {
            let span = e.span();
            add(e.state(), ExprKind::Unary(op, value), span)
        });
    unary.boxed()
}
fn control(expression: P<'_, ExprId>) -> P<'_, ExprId> {
    let pattern = pattern();
    let conditional = token(Kind::If)
        .then(nl())
        .ignore_then(expression.clone())
        .then_ignore(nl())
        .then_ignore(token(Kind::Then))
        .then_ignore(nl())
        .then(expression.clone())
        .then_ignore(nl())
        .then_ignore(token(Kind::Else))
        .then_ignore(nl())
        .then(expression.clone())
        .map_with(|((condition, yes), no), e| node(ExprKind::If { condition, yes, no }, e));
    let arm = pattern
        .clone()
        .then(
            token(Kind::If)
                .then(nl())
                .ignore_then(expression.clone())
                .or_not(),
        )
        .then_ignore(nl())
        .then_ignore(token(Kind::Arrow))
        .then_ignore(nl())
        .then(expression.clone())
        .map(|((pattern, guard), body)| Arm {
            pattern,
            guard,
            body,
        });
    let matching = token(Kind::Match)
        .then(nl())
        .ignore_then(expression)
        .then_ignore(nl())
        .then_ignore(token(Kind::Of))
        .then(
            arm.padded_by(nl())
                .separated_by(token(Kind::Comma))
                .allow_trailing()
                .collect::<Vec<_>>()
                .delimited_by(token(Kind::OpenBrace).then(nl()), token(Kind::CloseBrace)),
        )
        .map_with(|(subject, arms), e| node(ExprKind::Match { subject, arms }, e));
    conditional.or(matching).boxed()
}
fn binary(base: P<'_, ExprId>) -> P<'_, ExprId> {
    let mut operators = base;
    for level in [
        vec![
            (Kind::Star, Binary::Multiply),
            (Kind::Slash, Binary::Divide),
        ],
        vec![(Kind::Plus, Binary::Add), (Kind::Minus, Binary::Subtract)],
        vec![
            (Kind::Equal, Binary::Equal),
            (Kind::NotEqual, Binary::NotEqual),
            (Kind::Less, Binary::Less),
            (Kind::LessEqual, Binary::LessEqual),
            (Kind::Greater, Binary::Greater),
            (Kind::GreaterEqual, Binary::GreaterEqual),
        ],
        vec![(Kind::And, Binary::And)],
        vec![(Kind::Or, Binary::Or)],
        vec![(Kind::With, Binary::With)],
        vec![(Kind::ValuePipe, Binary::Pipe)],
    ] {
        let maximum = if level[0].0 == Kind::Equal {
            1
        } else {
            usize::MAX
        };
        let leading = if level[0].0 == Kind::ValuePipe {
            nl().ignored().boxed()
        } else {
            soft().ignored().boxed()
        };
        let op = leading.ignore_then(any()).try_map(move |t: Token, span| {
            level
                .iter()
                .find(|(kind, _)| *kind == t.kind)
                .map(|(_, op)| *op)
                .ok_or_else(|| Rich::custom(span, "expected operator"))
        });
        operators = operators
            .clone()
            .foldl_with(
                op.then_ignore(nl())
                    .then(operators)
                    .repeated()
                    .at_most(maximum),
                |left, (op, right), e| {
                    let span = e.span();
                    add(e.state(), ExprKind::Binary(op, left, right), span)
                },
            )
            .boxed();
    }
    operators
}
fn functions(expression: P<'_, ExprId>) -> P<'_, Statement> {
    let name = name();
    let separated_parameter = separated_parameter();
    let function = token(Kind::Fn)
        .ignore_then(name.clone())
        .then(
            separated_parameter
                .clone()
                .repeated()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(token(Kind::Assign))
        .then_ignore(nl())
        .then(expression)
        .map(|((name, parameters), body)| Function {
            name,
            parameters,
            body,
        });
    let function_group = token(Kind::Rec)
        .ignore_then(token(Kind::OpenBrace))
        .then(nl())
        .ignore_then(
            function
                .clone()
                .separated_by(separator())
                .allow_trailing()
                .at_least(1)
                .collect::<Vec<_>>(),
        )
        .then_ignore(token(Kind::CloseBrace))
        .map(Statement::Functions);
    function_group
        .or(function.map(|f| Statement::Functions(vec![f])))
        .boxed()
}
fn nominal<'a>() -> P<'a, Statement> {
    let name = name();
    let fields = name
        .clone()
        .padded_by(nl())
        .separated_by(token(Kind::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(token(Kind::OpenBrace).then(nl()), token(Kind::CloseBrace));
    let structure = token(Kind::Struct)
        .ignore_then(name.clone())
        .then(fields.clone())
        .map(|(name, fields)| Statement::Struct(name, fields));
    let enumeration = token(Kind::Enum)
        .ignore_then(name.clone())
        .then(
            name.clone()
                .then(fields.or_not())
                .map(|(name, fields)| (name, fields.unwrap_or_default()))
                .padded_by(nl())
                .separated_by(token(Kind::Comma))
                .allow_trailing()
                .at_least(1)
                .collect::<Vec<_>>()
                .delimited_by(token(Kind::OpenBrace).then(nl()), token(Kind::CloseBrace)),
        )
        .map(|(name, cases)| Statement::Enum(name, cases));
    structure.or(enumeration).boxed()
}
fn exports<'a>() -> P<'a, Statement> {
    let key = key();
    let path = path();
    let exports = key
        .then(token(Kind::Colon).ignore_then(path).or_not())
        .try_map(|((key, shorthand), path), span| {
            let path = path
                .or_else(|| shorthand.then(|| vec![key.clone()]))
                .ok_or_else(|| Rich::custom(span, "expected ':' and an export path"))?;
            Ok((key, path))
        })
        .padded_by(nl())
        .separated_by(token(Kind::Comma))
        .allow_trailing()
        .collect::<Vec<_>>()
        .delimited_by(token(Kind::OpenBrace).then(nl()), token(Kind::CloseBrace));
    token(Kind::Export)
        .ignore_then(exports)
        .map(Statement::Export)
        .boxed()
}
fn grammar<'a>() -> P<'a, Vec<Statement>> {
    // Nested `recursive` builders use weak back-references. Independently declared
    // parsers would retain one another through their owned definitions.
    recursive(|block| {
        let expr = recursive(|expression| {
            let expr = expression.boxed();
            let statements = block.clone().boxed();
            let plan = token(Kind::Job)
                .ignore_then(token(Kind::OpenBrace))
                .then(nl())
                .ignore_then(pipeline(expr.clone()))
                .then_ignore(nl())
                .then_ignore(token(Kind::CloseBrace))
                .map_with(|stages, e| node(ExprKind::Plan(stages), e));
            let atom = choice((
                closure(statements.clone()),
                aggregates(expr.clone(), statements),
                literal()
                    .map_with(|lit, e| node(ExprKind::Literal(lit), e))
                    .boxed(),
                name()
                    .try_map(|n, span| {
                        if n == "_" {
                            Err(Rich::custom(span, "'_' is reserved for patterns"))
                        } else {
                            Ok(n)
                        }
                    })
                    .map_with(|name, e| node(ExprKind::Name(name), e))
                    .boxed(),
                plan.boxed(),
            ))
            .boxed();
            binary(
                control(expr.clone())
                    .or(application(expr.clone(), atom))
                    .boxed(),
            )
        })
        .boxed();
        let statement = choice((
            token(Kind::Let)
                .ignore_then(pattern())
                .then_ignore(token(Kind::Assign))
                .then_ignore(nl())
                .then(expr.clone())
                .map(|(p, e)| Statement::Let(p, e))
                .boxed(),
            functions(expr.clone()),
            nominal(),
            exports(),
            token(Kind::Import)
                .ignore_then(string())
                .then_ignore(token(Kind::As))
                .then(name())
                .map(|(path, name)| Statement::Import { path, name })
                .boxed(),
            pipeline(expr.clone()).map(Statement::Command).boxed(),
            expr.map(Statement::Expression).boxed(),
        ));
        statement
            .separated_by(separator())
            .allow_trailing()
            .collect::<Vec<_>>()
            .padded_by(separator().or_not())
    })
    .boxed()
}

/// Parse an entire entry before any of its effects can run.
///
/// # Errors
/// Returns lexical, grammar, or structural binding diagnostics with byte spans.
pub fn parse(name: &str, source: &str) -> Result<Module, Vec<Diagnostic>> {
    let tokens = token::lex(source).map_err(|e| {
        vec![Diagnostic {
            span: e.span,
            message: e.message,
            incomplete: e.incomplete,
        }]
    })?;
    let spanned: Vec<_> = tokens
        .into_iter()
        .map(|t| {
            let span = SimpleSpan::from(t.span.clone());
            (t, span)
        })
        .collect();
    let input: TokenInput<'_> = spanned
        .as_slice()
        .map(SimpleSpan::from(source.len()..source.len()), |(t, span)| {
            (t, span)
        });
    let mut state = State::from(Vec::new());
    let statements = grammar()
        .parse_with_state(input, &mut state)
        .into_result()
        .map_err(|errors| {
            errors
                .into_iter()
                .map(|e| Diagnostic {
                    span: e.span().into_range(),
                    message: e.reason().to_string(),
                    incomplete: e.span().start >= source.len(),
                })
                .collect::<Vec<_>>()
        })?;
    let module = Module {
        name: name.into(),
        source: source.into(),
        statements,
        nodes: state.0,
    };
    crate::validate::validate(&module)?;
    Ok(module)
}
