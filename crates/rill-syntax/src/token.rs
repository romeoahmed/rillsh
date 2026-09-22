//! Contextual tokenization. Command words and expressions share quoted literals.

use logos::Logos;
use std::ops::Range;

/// A byte range in the original UTF-8 source.
pub type Span = Range<usize>;

/// A token with its original byte position and preceding horizontal separation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: Kind,
    pub span: Span,
    pub separated: bool,
}

/// Lexical categories; number spelling is checked by semantic conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Name(String),
    Number(String),
    String(String),
    Word(String),
    Let,
    Fn,
    Rec,
    If,
    Then,
    Else,
    Do,
    Match,
    Of,
    Struct,
    Enum,
    With,
    Plan,
    Import,
    As,
    Export,
    True,
    False,
    Null,
    And,
    Or,
    Not,
    OpenParen,
    CloseParen,
    OpenList,
    CloseList,
    OpenBrace,
    CloseBrace,
    Comma,
    Colon,
    Dot,
    Rest,
    Arrow,
    Assign,
    Plus,
    Minus,
    Star,
    Slash,
    Equal,
    NotEqual,
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Pipe,
    ValuePipe,
    Command,
    Substitute,
    Spread,
    Redirect(Redirect),
    Newline,
    Semicolon,
}

/// Ordered redirection operations supported by Rill command syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redirect {
    Input,
    Output,
    Append,
    ErrorOutput,
    ErrorAppend,
    ErrorToOutput,
}

/// A lexical failure distinguishes incomplete input from invalid spelling.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct LexError {
    pub span: Span,
    pub message: String,
    pub incomplete: bool,
}

#[derive(Logos, Clone, Copy, Debug, PartialEq, Eq)]
enum Expression {
    #[regex(r"[ \t\r]+")]
    Space,
    #[regex(r"#[^\n]*", allow_greedy = true)]
    Comment,
    #[token("\n")]
    Newline,
    #[regex(r"[A-Za-z_][A-Za-z0-9_]*")]
    Name,
    #[regex(r"[0-9]([0-9_]*[0-9])?(\.[0-9]([0-9_]*[0-9])?)?([eE][+-]?[0-9]([0-9_]*[0-9])?)?")]
    Number,
    #[token("\"")]
    DoubleQuote,
    #[token("'")]
    SingleQuote,
    #[token("(")]
    OpenParen,
    #[token(")")]
    CloseParen,
    #[token("[")]
    OpenList,
    #[token("]")]
    CloseList,
    #[token("{")]
    OpenBrace,
    #[token("}")]
    CloseBrace,
    #[token(",")]
    Comma,
    #[token(":")]
    Colon,
    #[token(".")]
    Dot,
    #[token("..")]
    Rest,
    #[token("=>")]
    Arrow,
    #[token("=")]
    Assign,
    #[token("+")]
    Plus,
    #[token("-")]
    Minus,
    #[token("*")]
    Star,
    #[token("/")]
    Slash,
    #[token("==")]
    Equal,
    #[token("!=")]
    NotEqual,
    #[token("<")]
    Less,
    #[token("<=")]
    LessEqual,
    #[token(">")]
    Greater,
    #[token(">=")]
    GreaterEqual,
    #[token("|>")]
    ValuePipe,
    #[token("^")]
    Command,
    #[token(";")]
    Semicolon,
    #[token("|")]
    Pipe,
}

#[derive(Logos, Clone, Copy, Debug, PartialEq, Eq)]
enum Command {
    #[regex(r"[ \t\r]+")]
    Space,
    #[token("\n")]
    Newline,
    #[token(";")]
    Semicolon,
    #[token("|")]
    Pipe,
    #[token("|>")]
    ValuePipe,
    #[token("^")]
    Caret,
    #[token("{")]
    OpenBrace,
    #[token("}")]
    CloseBrace,
    #[token("\"")]
    DoubleQuote,
    #[token("'")]
    SingleQuote,
    #[token("$")]
    Substitute,
    #[token("...$")]
    Spread,
    #[token("<")]
    Input,
    #[token(">")]
    Output,
    #[token(">>")]
    Append,
    #[token("2>")]
    ErrorOutput,
    #[token("2>>")]
    ErrorAppend,
    #[token("2>&1")]
    ErrorToOutput,
    #[regex(r#"[^ \t\r\n;|{}<>$'"^]+"#)]
    Word,
}

fn keyword(text: &str) -> Kind {
    match text {
        "let" => Kind::Let,
        "fn" => Kind::Fn,
        "rec" => Kind::Rec,
        "if" => Kind::If,
        "then" => Kind::Then,
        "else" => Kind::Else,
        "do" => Kind::Do,
        "match" => Kind::Match,
        "of" => Kind::Of,
        "struct" => Kind::Struct,
        "enum" => Kind::Enum,
        "with" => Kind::With,
        "plan" => Kind::Plan,
        "import" => Kind::Import,
        "as" => Kind::As,
        "export" => Kind::Export,
        "true" => Kind::True,
        "false" => Kind::False,
        "null" => Kind::Null,
        "and" => Kind::And,
        "or" => Kind::Or,
        "not" => Kind::Not,
        _ => Kind::Name(text.into()),
    }
}

/// Decode one quoted token, returning its exclusive end offset.
fn quoted(source: &str, start: usize, quote: char) -> Result<(String, usize), LexError> {
    let mut chars = source[start + 1..].char_indices();
    let mut output = String::new();
    while let Some((offset, ch)) = chars.next() {
        if ch == quote {
            return Ok((output, start + offset + 2));
        }
        if ch != '\\' || quote == '\'' {
            output.push(ch);
            continue;
        }
        let Some((escape_offset, escape)) = chars.next() else {
            break;
        };
        match escape {
            'n' => output.push('\n'),
            'r' => output.push('\r'),
            't' => output.push('\t'),
            '\\' => output.push('\\'),
            '"' => output.push('"'),
            'u' => {
                let Some((_, brace)) = chars.next() else {
                    return Err(incomplete(start..source.len(), "unfinished Unicode escape"));
                };
                if brace != '{' {
                    return Err(invalid(
                        start..start + offset + 3,
                        "expected '{' after Unicode escape",
                    ));
                }
                let mut digits = String::new();
                let mut closed = false;
                for (_, digit) in chars.by_ref() {
                    if digit == '}' {
                        closed = true;
                        break;
                    }
                    if !digit.is_ascii_hexdigit() || digits.len() >= 6 {
                        return Err(invalid(
                            start..source.len(),
                            "invalid Unicode scalar escape",
                        ));
                    }
                    digits.push(digit);
                }
                if !closed {
                    return Err(incomplete(start..source.len(), "unfinished Unicode escape"));
                }
                let value = u32::from_str_radix(&digits, 16)
                    .ok()
                    .and_then(char::from_u32);
                let Some(value) = value else {
                    return Err(invalid(
                        start..source.len(),
                        "invalid Unicode scalar escape",
                    ));
                };
                output.push(value);
            }
            _ => {
                return Err(invalid(
                    start..start + 1 + escape_offset + escape.len_utf8(),
                    "unknown string escape",
                ));
            }
        }
    }
    Err(incomplete(start..source.len(), "unterminated string"))
}

fn invalid(span: Span, message: &str) -> LexError {
    LexError {
        span,
        message: message.into(),
        incomplete: false,
    }
}
fn incomplete(span: Span, message: &str) -> LexError {
    LexError {
        span,
        message: message.into(),
        incomplete: true,
    }
}

#[derive(Clone, Copy)]
enum Mode {
    Expression,
    Command,
}

fn expression_token(source: &str, start: usize) -> Result<(Option<Kind>, usize), LexError> {
    let mut lexer = Expression::lexer(&source[start..]);
    let token = lexer.next().transpose().map_err(|()| {
        invalid(
            start..start + source[start..].chars().next().map_or(0, char::len_utf8),
            "invalid expression character",
        )
    })?;
    let Some(token) = token else {
        return Ok((None, source.len()));
    };
    let mut position = start + lexer.span().end;
    let kind = match token {
        Expression::Space | Expression::Comment => {
            return Ok((None, position));
        }
        Expression::Newline => Kind::Newline,
        Expression::Name => keyword(lexer.slice()),
        Expression::Number => Kind::Number(lexer.slice().into()),
        Expression::DoubleQuote | Expression::SingleQuote => {
            let (text, end) = quoted(
                source,
                start,
                if token == Expression::DoubleQuote {
                    '"'
                } else {
                    '\''
                },
            )?;
            position = end;
            Kind::String(text)
        }
        Expression::OpenParen => Kind::OpenParen,
        Expression::CloseParen => Kind::CloseParen,
        Expression::OpenList => Kind::OpenList,
        Expression::CloseList => Kind::CloseList,
        Expression::OpenBrace => Kind::OpenBrace,
        Expression::CloseBrace => Kind::CloseBrace,
        Expression::Comma => Kind::Comma,
        Expression::Colon => Kind::Colon,
        Expression::Dot => Kind::Dot,
        Expression::Rest => Kind::Rest,
        Expression::Arrow => Kind::Arrow,
        Expression::Assign => Kind::Assign,
        Expression::Plus => Kind::Plus,
        Expression::Minus => Kind::Minus,
        Expression::Star => Kind::Star,
        Expression::Slash => Kind::Slash,
        Expression::Equal => Kind::Equal,
        Expression::NotEqual => Kind::NotEqual,
        Expression::Less => Kind::Less,
        Expression::LessEqual => Kind::LessEqual,
        Expression::Greater => Kind::Greater,
        Expression::GreaterEqual => Kind::GreaterEqual,
        Expression::ValuePipe => Kind::ValuePipe,
        Expression::Command => Kind::Command,
        Expression::Semicolon => Kind::Semicolon,
        Expression::Pipe => Kind::Pipe,
    };
    Ok((Some(kind), position))
}
fn command_token(source: &str, start: usize) -> Result<(Option<Kind>, usize), LexError> {
    let mut lexer = Command::lexer(&source[start..]);
    let token = lexer.next().transpose().map_err(|()| {
        invalid(
            start..start + source[start..].chars().next().map_or(0, char::len_utf8),
            "invalid command character",
        )
    })?;
    let Some(token) = token else {
        return Ok((None, source.len()));
    };
    let mut position = start + lexer.span().end;
    let kind = match token {
        Command::Space => {
            return Ok((None, position));
        }
        Command::Newline => Kind::Newline,
        Command::Semicolon => Kind::Semicolon,
        Command::Pipe => Kind::Pipe,
        Command::ValuePipe => Kind::ValuePipe,
        Command::Caret => Kind::Command,
        Command::OpenBrace => Kind::OpenBrace,
        Command::CloseBrace => Kind::CloseBrace,
        Command::DoubleQuote | Command::SingleQuote => {
            let (text, end) = quoted(
                source,
                start,
                if token == Command::DoubleQuote {
                    '"'
                } else {
                    '\''
                },
            )?;
            position = end;
            Kind::String(text)
        }
        Command::Substitute | Command::Spread => {
            if token == Command::Spread {
                Kind::Spread
            } else {
                Kind::Substitute
            }
        }
        Command::Input => Kind::Redirect(Redirect::Input),
        Command::Output => Kind::Redirect(Redirect::Output),
        Command::Append => Kind::Redirect(Redirect::Append),
        Command::ErrorOutput => Kind::Redirect(Redirect::ErrorOutput),
        Command::ErrorAppend => Kind::Redirect(Redirect::ErrorAppend),
        Command::ErrorToOutput => Kind::Redirect(Redirect::ErrorToOutput),
        Command::Word => {
            if lexer.slice().bytes().all(|b| b.is_ascii_digit())
                && lexer.remainder().starts_with(['<', '>'])
            {
                return Err(invalid(
                    start..position,
                    "unsupported descriptor redirection",
                ));
            }
            Kind::Word(lexer.slice().into())
        }
    };
    Ok((Some(kind), position))
}
/// Tokenize source without evaluating names or consulting parser backtracking.
///
/// # Errors
/// Reports invalid characters, malformed escapes, or unfinished substitutions.
pub fn lex(source: &str) -> Result<Vec<Token>, LexError> {
    let mut tokens = Vec::new();
    lex_into(source, &mut tokens)?;
    Ok(tokens)
}
/// Preserve the valid prefix for highlighting unfinished or invalid input.
#[must_use]
pub fn highlight_tokens(source: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let _ = lex_into(source, &mut tokens);
    tokens
}
fn lex_into(source: &str, tokens: &mut Vec<Token>) -> Result<(), LexError> {
    let mut position = 0;
    let mut mode = Mode::Expression;
    let mut contexts: Vec<(Kind, Mode)> = Vec::new();
    let mut separated = false;
    let mut substitution = false;
    while position < source.len() {
        let start = position;
        let (kind, end) = match mode {
            Mode::Expression => expression_token(source, start)?,
            Mode::Command => {
                if source[start..].starts_with('#') && separated {
                    position += source[start..].find('\n').unwrap_or(source.len() - start);
                    continue;
                }
                command_token(source, start)?
            }
        };
        position = end;
        let Some(kind) = kind else {
            separated = true;
            continue;
        };
        if matches!(kind, Kind::Substitute | Kind::Spread) {
            substitution = true;
        }
        if substitution && !matches!(kind, Kind::Substitute | Kind::Spread) {
            substitution = false;
            if !matches!(kind, Kind::OpenParen | Kind::Name(_)) || separated {
                return Err(invalid(
                    start..position,
                    "expected a name or parenthesized substitution",
                ));
            }
            mode = Mode::Command;
        }
        match kind {
            Kind::OpenParen | Kind::OpenList | Kind::OpenBrace => {
                contexts.push((kind.clone(), mode));
                mode = Mode::Expression;
                if contexts.len() > 256 {
                    return Err(invalid(
                        start..position,
                        "syntax nesting exceeds 256 levels",
                    ));
                }
            }
            Kind::CloseParen | Kind::CloseList | Kind::CloseBrace => {
                let expected = match kind {
                    Kind::CloseParen => Kind::OpenParen,
                    Kind::CloseList => Kind::OpenList,
                    _ => Kind::OpenBrace,
                };
                let Some((open, parent)) = contexts.pop() else {
                    return Err(invalid(start..position, "unmatched closing delimiter"));
                };
                if open != expected {
                    return Err(invalid(start..position, "mismatched closing delimiter"));
                }
                mode = parent;
            }
            Kind::Command => mode = Mode::Command,
            Kind::Substitute | Kind::Spread | Kind::Newline | Kind::Semicolon => {
                mode = Mode::Expression;
            }
            _ => {}
        }
        tokens.push(Token {
            kind,
            span: start..position,
            separated,
        });
        separated = false;
    }
    if substitution {
        return Err(incomplete(position..position, "unfinished substitution"));
    }
    Ok(())
}

impl Kind {
    /// Identifier spelling for data keys, including reserved words.
    #[must_use]
    pub fn field_name(&self) -> Option<&str> {
        match self {
            Self::Name(name) => Some(name),
            Self::Let => Some("let"),
            Self::Fn => Some("fn"),
            Self::Rec => Some("rec"),
            Self::If => Some("if"),
            Self::Then => Some("then"),
            Self::Else => Some("else"),
            Self::Do => Some("do"),
            Self::Match => Some("match"),
            Self::Of => Some("of"),
            Self::Struct => Some("struct"),
            Self::Enum => Some("enum"),
            Self::With => Some("with"),
            Self::Plan => Some("plan"),
            Self::Import => Some("import"),
            Self::As => Some("as"),
            Self::Export => Some("export"),
            Self::True => Some("true"),
            Self::False => Some("false"),
            Self::Null => Some("null"),
            Self::And => Some("and"),
            Self::Or => Some("or"),
            Self::Not => Some("not"),
            _ => None,
        }
    }
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(name) = self.kind.field_name() {
            return write!(f, "'{name}'");
        }
        let spelling = match &self.kind {
            Kind::Number(number) => return f.write_str(number),
            Kind::Word(word) => return write!(f, "{word:?}"),
            Kind::String(_) => return f.write_str("string literal"),
            Kind::Newline => return f.write_str("newline"),
            Kind::OpenParen => "(",
            Kind::CloseParen => ")",
            Kind::OpenList => "[",
            Kind::CloseList => "]",
            Kind::OpenBrace => "{",
            Kind::CloseBrace => "}",
            Kind::Comma => ",",
            Kind::Colon => ":",
            Kind::Dot => ".",
            Kind::Rest => "..",
            Kind::Arrow => "=>",
            Kind::Assign => "=",
            Kind::Plus => "+",
            Kind::Minus => "-",
            Kind::Star => "*",
            Kind::Slash => "/",
            Kind::Equal => "==",
            Kind::NotEqual => "!=",
            Kind::Less => "<",
            Kind::LessEqual => "<=",
            Kind::Greater => ">",
            Kind::GreaterEqual => ">=",
            Kind::Pipe => "|",
            Kind::ValuePipe => "|>",
            Kind::Command => "^",
            Kind::Substitute => "$",
            Kind::Spread => "...$",
            Kind::Semicolon => ";",
            Kind::Redirect(redirect) => match redirect {
                Redirect::Input => "<",
                Redirect::Output => ">",
                Redirect::Append => ">>",
                Redirect::ErrorOutput => "2>",
                Redirect::ErrorAppend => "2>>",
                Redirect::ErrorToOutput => "2>&1",
            },
            _ => unreachable!("identifier and keyword spelling handled above"),
        };
        write!(f, "'{spelling}'")
    }
}
