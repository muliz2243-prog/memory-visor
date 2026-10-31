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
    match analyze_program(source) {
        Analysis::Analyzed(program) => record_program(source, program),
        Analysis::Failed(error) => {
            if !error.detail.contains("missing 'main'") {
                return Err(RecordError::new(error.line, error.detail));
            }
            // No `main`: the buffer itself is the body, so a bare sequence of
            // statements still records instead of demanding a function header.
            let wrapped = format!("fn main() {{\n{}\n}}", source);
            match analyze_program(&wrapped) {
                Analysis::Analyzed(program) => record_body(source, program),
                Analysis::Failed(error) => Err(RecordError::new(error.line, error.detail)),
            }
        }
    }
}

fn record_program(source: &str, program: ProgramTree) -> Result<Recording, RecordError> {
    let main = match program.function_named("main") {
        Some(main) => main,
        None => return Err(RecordError::new(1, "the program needs a main function")),
    };
    let mut builder = Builder::new(&program, main);
    for (index, raw) in source.lines().enumerate() {
        builder.line(index as u32 + 1, raw.trim())?;
    }
    builder.finish()
}

fn record_body(source: &str, program: ProgramTree) -> Result<Recording, RecordError> {
    let main = match program.function_named("main") {
        Some(main) => main,
        None => return Err(RecordError::new(1, "the program needs a main function")),
    };
    let mut builder = Builder::new(&program, main);
    builder.start(1);
    for (index, raw) in source.lines().enumerate() {
        builder.line(index as u32 + 1, raw.trim())?;
    }
    builder.close_remaining();
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
    /// `main` has started.
    started: bool,
    /// `main` has closed; anything after it is not part of the run.
    finished: bool,
    /// Brace depth inside a struct or a function that is not `main`.
    skip: i32,
}

impl<'a> Builder<'a> {
    fn new(program: &'a ProgramTree, main: crate::model::ids::FunctionId) -> Self {
        Builder {
            program,
            main,
            events: Vec::new(),
            sequence: 0,
            scopes: Vec::new(),
            next_scope: 1,
            variables: Vec::new(),
            started: false,
            finished: false,
            skip: 0,
        }
    }

    fn line(&mut self, number: u32, text: &str) -> Result<(), RecordError> {
        if text.is_empty() || text.starts_with("//") {
            return Ok(());
        }
        let (opens, closes) = braces(text);
        if self.skip > 0 {
            self.skip = (self.skip + opens as i32 - closes as i32).max(0);
            return Ok(());
        }
        if text.starts_with("fn ") {
            self.function_header(number, text, opens as i32 - closes as i32);
            return Ok(());
        }
        if text.starts_with("struct ") {
            self.skip = (opens as i32 - closes as i32).max(0);
            return Ok(());
        }
        if !self.started || self.finished {
            return Ok(());
        }
        if let Some(rest) = text.strip_prefix("let ") {
            self.declare(number, rest);
            return Ok(());
        }
        self.walk_braces(number, opens, closes);
        Ok(())
    }

    /// Enters `main` when it appears, wherever it is; every other function body
    /// is skipped so its braces never touch the recorded scopes.
    fn function_header(&mut self, number: u32, text: &str, net: i32) {
        if self.started || !text.starts_with("fn main(") {
            self.skip = net.max(0);
            return;
        }
        self.start(number);
    }

    fn start(&mut self, line: u32) {
        self.started = true;
        let function = self.main;
        let sequence = self.next_sequence();
        self.push(TraceEvent::StartFunction {
            sequence,
            function,
            call_line: line,
            arguments: Vec::new(),
        });
        self.enter_scope(line, ScopeKind::Function);
    }

    fn walk_braces(&mut self, number: u32, opens: usize, closes: usize) {
        for _ in 0..closes {
            self.close_scope();
        }
        for _ in 0..opens {
            self.enter_scope(number, ScopeKind::Block);
        }
    }

    fn enter_scope(&mut self, number: u32, kind: ScopeKind) {
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
    }

    /// Closes the innermost scope; a stray `}` is ignored instead of rejected.
    fn close_scope(&mut self) {
        let scope = match self.scopes.pop() {
            Some(scope) => scope,
            None => return,
        };
        let sequence = self.next_sequence();
        self.push(TraceEvent::ExitScope { sequence, scope });
        let depth = self.scopes.len();
        self.variables.retain(|binding| binding.depth <= depth);
        if self.scopes.is_empty() {
            self.finished = true;
        }
    }

    /// Records a binding when its initializer can be evaluated. Anything the
    /// evaluator cannot reduce (a call, a struct literal, a missing initializer)
    /// is skipped instead of rejecting the whole buffer: the rest of the trace
    /// stays useful while the line is still being typed.
    fn declare(&mut self, number: u32, rest: &str) {
        if self.scopes.is_empty() {
            return;
        }
        let statement = rest.trim_end().trim_end_matches(';');
        let Some((name, value)) = statement.split_once(':') else {
            return;
        };
        let name = name.trim().trim_start_matches("mut ").trim();
        let Some((_, expression)) = value.split_once('=') else {
            return;
        };
        let Some(declaration) = self.program.find_declaration(self.main, name) else {
            return;
        };
        let Some(value) = evaluate(&declaration.data_type, expression.trim(), &self.variables)
        else {
            return;
        };
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
    }

    /// Closes whatever the synthetic `main` left open: the buffer is a body,
    /// so its scopes end at the end of the file.
    fn close_remaining(&mut self) {
        while !self.scopes.is_empty() {
            self.close_scope();
        }
    }

    fn finish(mut self) -> Result<Recording, RecordError> {
        if !self.started {
            return Err(RecordError::new(1, "the program needs a main function"));
        }
        if !self.scopes.is_empty() {
            let line = self.last_line();
            return Err(RecordError::new(line, "unclosed `{`"));
        }
        let sequence = self.next_sequence();
        self.push(TraceEvent::EndFunction {
            sequence,
            return_value: None,
        });
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

/// Opening and closing braces in one line. String literals with braces are
/// out of the recorded subset anyway, so counting characters is enough.
fn braces(text: &str) -> (usize, usize) {
    let opens = text.matches('{').count();
    let closes = text.matches('}').count();
    (opens, closes)
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
    fn statements_that_are_not_memory_events_are_ignored() {
        let recording = record("fn main() {\n let a: i32 = 1;\n println!(\"hi\");\n}")
            .expect("calls must not block the trace");
        assert_eq!(
            declaration_value(&recording, "a"),
            PrimitiveValue::Integer(1)
        );
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
    fn main_may_come_after_other_functions() {
        let source =
            "fn helper(x: i32) -> i32 {\n let y: i32 = x;\n}\nfn main() {\n let a: i32 = 2;\n}";
        let recording = record(source).expect("main need not be first");
        assert_eq!(
            declaration_value(&recording, "a"),
            PrimitiveValue::Integer(2)
        );
        assert!(
            recording.events.iter().all(
                |event| !matches!(event, TraceEvent::DeclareVariable { name, .. } if name == "y")
            ),
            "the helper body must not run"
        );
    }

    #[test]
    fn a_bare_sequence_of_statements_records_without_a_main() {
        let recording = record("let a: i32 = 1;\nlet b: i32 = a + 2;")
            .expect("top-level statements must record");
        assert_eq!(
            declaration_value(&recording, "b"),
            PrimitiveValue::Integer(3)
        );
        assert!(matches!(
            recording.events.first(),
            Some(TraceEvent::StartFunction { .. })
        ));
    }

    #[test]
    fn a_binding_that_cannot_be_evaluated_is_skipped() {
        let recording = record("let a: i32 = ;\nlet b: i32 = 2;")
            .expect("an unreadable initializer must not block the trace");
        assert!(
            !recording.events.iter().any(
                |event| matches!(event, TraceEvent::DeclareVariable { name, .. } if name == "a")
            ),
            "the unreadable binding records nothing"
        );
        assert_eq!(
            declaration_value(&recording, "b"),
            PrimitiveValue::Integer(2)
        );
    }
}
