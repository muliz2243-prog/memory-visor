//! Source text -> the events the window replays.
//!
//! The recorder covers the subset the canvas can explain: one `main` with
//! typed `let` bindings, arithmetic over earlier bindings and nested `{}`
//! blocks. Every
//! block emits `EnterScope`/`ExitScope`, so the reducer produces the same LIFO
//! drops as a real run. Anything outside the subset is rejected with its line
//! instead of guessed.

use crate::model::events::TraceEvent;
use crate::model::ids::{ScopeId, Sequence};
use crate::model::memory::{PrimitiveValue, Value};
use crate::model::program::{DataType, ProgramTree};
use crate::model::scope::ScopeKind;
use crate::stages::analyze::{Analysis, analyze_program};

/// A program and the events that reproduce it.
#[derive(Clone, PartialEq, Debug)]
pub struct Recording {
    pub program: ProgramTree,
    pub events: Vec<TraceEvent>,
}

/// Something the recorder cannot translate, pinned to a line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RecordError {
    pub line: u32,
    pub detail: String,
}

impl RecordError {
    pub fn new(line: u32, detail: impl Into<String>) -> Self {
        RecordError {
            line,
            detail: detail.into(),
        }
    }
}

pub fn record(source: &str) -> Result<Recording, RecordError> {
    let program = match analyze_program(source) {
        Analysis::Analyzed(program) => program,
        Analysis::Failed(error) => return Err(RecordError::new(error.line, error.detail)),
    };
    let main = match program.function_named("main") {
        Some(main) => main,
        None => return Err(RecordError::new(1, "the program needs a main function")),
    };
    let mut builder = Builder {
        program: &program,
        main,
        events: Vec::new(),
        sequence: 0,
        scopes: Vec::new(),
        next_scope: 1,
        variables: Vec::new(),
        started: false,
        finished: false,
    };
    for (index, raw) in source.lines().enumerate() {
        builder.line(index as u32 + 1, raw.trim())?;
    }
    builder.finish()
}

struct Builder<'a> {
    program: &'a ProgramTree,
    main: crate::model::ids::FunctionId,
    events: Vec<TraceEvent>,
    sequence: u64,
    scopes: Vec<ScopeId>,
    next_scope: u32,
    /// Values already declared, with the depth of the scope that owns them.
    variables: Vec<Binding>,
    started: bool,
    finished: bool,
}

impl Builder<'_> {
    fn line(&mut self, number: u32, text: &str) -> Result<(), RecordError> {
        if text.is_empty() || text.starts_with("//") {
            return Ok(());
        }
        if text.starts_with("fn ") {
            return self.start_function(number, text);
        }
        if text == "{" {
            return self.open_block(number);
        }
        if text == "}" {
            return self.close_scope(number);
        }
        if let Some(rest) = text.strip_prefix("let ") {
            return self.declare(number, rest);
        }
        Err(RecordError::new(
            number,
            "only `fn main`, typed `let` bindings and `{}` blocks are recorded",
        ))
    }

    fn start_function(&mut self, number: u32, text: &str) -> Result<(), RecordError> {
        if self.started {
            return Err(RecordError::new(number, "only one function can run"));
        }
        if !text.starts_with("fn main(") {
            return Err(RecordError::new(number, "only `main` can run"));
        }
        self.started = true;
        let function = self.main;
        let sequence = self.next_sequence();
        self.push(TraceEvent::StartFunction {
            sequence,
            function,
            call_line: number,
            arguments: Vec::new(),
        });
        self.enter_scope(number, ScopeKind::Function)
    }

    fn open_block(&mut self, number: u32) -> Result<(), RecordError> {
        if !self.started {
            return Err(RecordError::new(number, "a block needs a running function"));
        }
        self.enter_scope(number, ScopeKind::Block)
    }

    fn enter_scope(&mut self, number: u32, kind: ScopeKind) -> Result<(), RecordError> {
        let scope = ScopeId(self.next_scope);
        self.next_scope += 1;
        let parent = self.scopes.last().copied();
        self.scopes.push(scope);
        let sequence = self.next_sequence();
        self.push(TraceEvent::EnterScope {
            sequence,
            line: number,
            scope,
            kind,
            parent,
        });
        Ok(())
    }

    fn close_scope(&mut self, number: u32) -> Result<(), RecordError> {
        let scope = match self.scopes.pop() {
            Some(scope) => scope,
            None => return Err(RecordError::new(number, "unmatched `}`")),
        };
        let sequence = self.next_sequence();
        self.push(TraceEvent::ExitScope { sequence, scope });
        let depth = self.scopes.len();
        self.variables.retain(|binding| binding.depth <= depth);
        Ok(())
    }

    fn declare(&mut self, number: u32, rest: &str) -> Result<(), RecordError> {
        if self.scopes.is_empty() {
            return Err(RecordError::new(number, "a binding needs an open scope"));
        }
        let statement = rest.trim_end().trim_end_matches(';');
        let (name, value) = statement
            .split_once(':')
            .ok_or_else(|| RecordError::new(number, "expected `let name: Type = value;`"))?;
        let name = name.trim();
        let (_, expression) = value
            .split_once('=')
            .ok_or_else(|| RecordError::new(number, "the binding needs an initializer"))?;
        let declaration = self
            .program
            .find_declaration(self.main, name)
            .ok_or_else(|| RecordError::new(number, "unknown declaration"))?;
        let value = evaluate(&declaration.data_type, expression.trim(), &self.variables)
            .ok_or_else(|| RecordError::new(number, "only literals and arithmetic are recorded"))?;
        let sequence = self.next_sequence();
        self.push(TraceEvent::DeclareVariable {
            sequence,
            line: number,
            name: name.to_string(),
            value: Value::Primitive(value.clone()),
        });
        self.variables.push(Binding {
            name: name.to_string(),
            value,
            depth: self.scopes.len(),
        });
        Ok(())
    }

    fn finish(mut self) -> Result<Recording, RecordError> {
        if !self.started {
            return Err(RecordError::new(1, "the program needs a main function"));
        }
        if !self.scopes.is_empty() {
            let line = self.last_line();
            return Err(RecordError::new(line, "unclosed `{`"));
        }
        if !self.finished {
            self.finished = true;
            let sequence = self.next_sequence();
            self.push(TraceEvent::EndFunction {
                sequence,
                return_value: None,
            });
        }
        Ok(Recording {
            program: self.program.clone(),
            events: self.events,
        })
    }

    fn last_line(&self) -> u32 {
        self.events
            .iter()
            .rev()
            .find_map(|event| event.active_line())
            .unwrap_or(1)
    }

    fn next_sequence(&mut self) -> Sequence {
        self.sequence += 1;
        Sequence(self.sequence)
    }

    fn push(&mut self, event: TraceEvent) {
        self.events.push(event);
    }
}

/// A declared value and the scope depth it lives in.
struct Binding {
    name: String,
    value: PrimitiveValue,
    depth: usize,
}

fn evaluate(data_type: &DataType, text: &str, variables: &[Binding]) -> Option<PrimitiveValue> {
    if let Some(binding) = lookup(variables, text) {
        return (data_type.kind() == binding.kind()).then(|| binding.clone());
    }
    match data_type {
        DataType::Integer => Some(PrimitiveValue::Integer(
            arithmetic(text, variables)?.as_integer()?,
        )),
        DataType::Float => Some(PrimitiveValue::Float(
            arithmetic(text, variables)?.as_float(),
        )),
        DataType::Boolean => match text {
            "true" => Some(PrimitiveValue::Boolean(true)),
            "false" => Some(PrimitiveValue::Boolean(false)),
            _ => None,
        },
        DataType::Character => Some(PrimitiveValue::Character(character(text)?)),
        DataType::Text => Some(PrimitiveValue::Text(
            text.strip_prefix('"')?.strip_suffix('"')?.to_string(),
        )),
        DataType::Structure(_) | DataType::Sequence(_) => None,
    }
}

fn lookup<'a>(variables: &'a [Binding], name: &str) -> Option<&'a PrimitiveValue> {
    variables
        .iter()
        .rev()
        .find(|binding| binding.name == name)
        .map(|binding| &binding.value)
}

fn character(text: &str) -> Option<char> {
    let inner = text.strip_prefix('\'')?.strip_suffix('\'')?;
    let mut chars = inner.chars();
    match (chars.next(), chars.next()) {
        (Some(value), None) => Some(value),
        _ => None,
    }
}

/// An integer that widens to a float only when a division leaves the integers.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Number {
    Integer(i64),
    Float(f64),
}

impl Number {
    fn as_float(self) -> f64 {
        match self {
            Number::Integer(value) => value as f64,
            Number::Float(value) => value,
        }
    }

    fn as_integer(self) -> Option<i64> {
        match self {
            Number::Integer(value) => Some(value),
            Number::Float(_) => None,
        }
    }

    fn add(self, other: Number) -> Option<Number> {
        match (self, other) {
            (Number::Integer(left), Number::Integer(right)) => {
                left.checked_add(right).map(Number::Integer)
            }
            _ => Some(Number::Float(self.as_float() + other.as_float())),
        }
    }

    fn sub(self, other: Number) -> Option<Number> {
        match (self, other) {
            (Number::Integer(left), Number::Integer(right)) => {
                left.checked_sub(right).map(Number::Integer)
            }
            _ => Some(Number::Float(self.as_float() - other.as_float())),
        }
    }

    fn mul(self, other: Number) -> Option<Number> {
        match (self, other) {
            (Number::Integer(left), Number::Integer(right)) => {
                left.checked_mul(right).map(Number::Integer)
            }
            _ => Some(Number::Float(self.as_float() * other.as_float())),
        }
    }

    fn div(self, other: Number) -> Option<Number> {
        match (self, other) {
            (Number::Integer(_), Number::Integer(0)) => None,
            (Number::Integer(left), Number::Integer(right)) => {
                left.checked_div(right).map(Number::Integer)
            }
            _ => Some(Number::Float(self.as_float() / other.as_float())),
        }
    }

    fn negate(self) -> Option<Number> {
        match self {
            Number::Integer(value) => value.checked_neg().map(Number::Integer),
            Number::Float(value) => Some(Number::Float(-value)),
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
enum Token {
    Number(String),
    Name(String),
    Plus,
    Minus,
    Star,
    Slash,
    Open,
    Close,
}

fn tokenize(text: &str) -> Option<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(character) = chars.next() {
        match character {
            ' ' | '\t' => {}
            '.' | '0'..='9' => {
                let mut number = character.to_string();
                while let Some(next) = chars.peek().copied() {
                    if next.is_ascii_digit() || next == '.' {
                        number.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Number(number));
            }
            'a'..='z' | 'A'..='Z' | '_' => {
                let mut name = character.to_string();
                while let Some(next) = chars.peek().copied() {
                    if next.is_alphanumeric() || next == '_' {
                        name.push(next);
                        chars.next();
                    } else {
                        break;
                    }
                }
                tokens.push(Token::Name(name));
            }
            '+' => tokens.push(Token::Plus),
            '-' => tokens.push(Token::Minus),
            '*' => tokens.push(Token::Star),
            '/' => tokens.push(Token::Slash),
            '(' => tokens.push(Token::Open),
            ')' => tokens.push(Token::Close),
            _ => return None,
        }
    }
    Some(tokens)
}

fn arithmetic(text: &str, variables: &[Binding]) -> Option<Number> {
    let tokens = tokenize(text)?;
    let mut parser = Parser {
        tokens,
        at: 0,
        variables,
    };
    let value = parser.expression()?;
    (parser.at == parser.tokens.len()).then_some(value)
}

struct Parser<'a> {
    tokens: Vec<Token>,
    at: usize,
    variables: &'a [Binding],
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned()?;
        self.at += 1;
        Some(token)
    }

    fn expression(&mut self) -> Option<Number> {
        let mut value = self.term()?;
        loop {
            value = match self.peek() {
                Some(Token::Plus) => {
                    self.at += 1;
                    value.add(self.term()?)
                }
                Some(Token::Minus) => {
                    self.at += 1;
                    value.sub(self.term()?)
                }
                _ => return Some(value),
            }?;
        }
    }

    fn term(&mut self) -> Option<Number> {
        let mut value = self.factor()?;
        loop {
            value = match self.peek() {
                Some(Token::Star) => {
                    self.at += 1;
                    value.mul(self.factor()?)
                }
                Some(Token::Slash) => {
                    self.at += 1;
                    value.div(self.factor()?)
                }
                _ => return Some(value),
            }?;
        }
    }

    fn factor(&mut self) -> Option<Number> {
        match self.bump()? {
            Token::Minus => self.factor()?.negate(),
            Token::Open => {
                let value = self.expression()?;
                match self.bump() {
                    Some(Token::Close) => Some(value),
                    _ => None,
                }
            }
            Token::Number(text) => match text.contains('.') {
                true => text.parse().ok().map(Number::Float),
                false => text.parse().ok().map(Number::Integer),
            },
            Token::Name(name) => match lookup(self.variables, &name)? {
                PrimitiveValue::Integer(value) => Some(Number::Integer(*value)),
                PrimitiveValue::Float(value) => Some(Number::Float(*value)),
                _ => None,
            },
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::edge::sample::LIFECYCLE_SOURCE;

    fn declaration(recording: &Recording, name: &str) -> Value {
        recording
            .events
            .iter()
            .find_map(|event| match event {
                TraceEvent::DeclareVariable {
                    name: declared,
                    value,
                    ..
                } if declared == name => Some(value.clone()),
                _ => None,
            })
            .unwrap_or_else(|| panic!("{} must be declared", name))
    }

    #[test]
    fn the_lifecycle_scene_records_and_evaluates_arithmetic() {
        let recording = record(LIFECYCLE_SOURCE).expect("the demo must record");
        assert_eq!(
            declaration(&recording, "result"),
            Value::Primitive(PrimitiveValue::Integer(8))
        );
    }

    #[test]
    fn a_block_opens_and_closes_around_its_bindings() {
        let recording = record("fn main() {\n let a: i32 = 1;\n {\n let b: i32 = a + 1;\n }\n}")
            .expect("nested blocks must record");
        let inner = declaration_value(&recording, "b");
        assert_eq!(inner, PrimitiveValue::Integer(2));
        let scopes: Vec<ScopeId> = recording
            .events
            .iter()
            .filter_map(|event| match event {
                TraceEvent::ExitScope { scope, .. } => Some(*scope),
                _ => None,
            })
            .collect();
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0], ScopeId(2), "the block closes first");
        assert_eq!(scopes[1], ScopeId(1), "then the function");
    }

    fn declaration_value(recording: &Recording, name: &str) -> PrimitiveValue {
        match declaration(recording, name) {
            Value::Primitive(value) => value,
            other => panic!("{} must be primitive, got {:?}", name, other),
        }
    }

    #[test]
    fn the_recorder_rejects_statements_outside_the_subset() {
        let error = record("fn main() {\n let a: i32 = 1;\n println!(\"hi\");\n}")
            .expect_err("calls are not recorded");
        assert_eq!(error.line, 3);
    }

    #[test]
    fn a_binding_may_use_an_earlier_binding() {
        let recording = record("fn main() {\n let a: i32 = 4;\n let b: i32 = a * a - 1;\n}")
            .expect("arithmetic over bindings must record");
        assert_eq!(
            declaration_value(&recording, "b"),
            PrimitiveValue::Integer(15)
        );
    }

    #[test]
    fn the_recorder_reports_a_missing_main() {
        let error = record("fn other() {\n}").expect_err("main is required");
        assert!(error.detail.contains("main"), "got {:?}", error.detail);
    }
}
