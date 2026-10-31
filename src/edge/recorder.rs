//! Source text -> the events the window replays.
//!
//! The recorder is a small tree-walking interpreter over the subset the canvas
//! can explain: functions with parameters and return values, typed `let`
//! bindings (`mut` or not), arithmetic, comparisons, `if`/`else`, nested blocks
//! and function calls. Every statement it runs becomes the event the reducer
//! would have produced, so a call pushes a frame, a mutation is a
//! `WriteMemory` and leaving a block is an `ExitScope`.
//!
//! It never guesses: a construct it cannot model (`struct` literals, arrays,
//! method calls, loops) stops the recording with its line instead of drawing a
//! trace that would be wrong.

use std::collections::HashMap;

use crate::model::events::{ConditionOutcome, TraceEvent};
use crate::model::ids::{FrameId, FunctionId, ScopeId, Sequence};
use crate::model::memory::{Field, Path, PrimitiveValue, Value};
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

/// Deepest call stack the recorder will follow, below the model's own limit.
const MAX_DEPTH: usize = 64;

pub fn record(source: &str) -> Result<Recording, RecordError> {
    match analyze_program(source) {
        Analysis::Analyzed(program) => run(source, program),
        Analysis::Failed(error) => {
            if !error.detail.contains("missing 'main'") {
                return Err(RecordError::new(error.line, error.detail));
            }
            // No `main`: the buffer itself is the body of an implicit `main`,
            // so a bare sequence of statements still records.
            let wrapped = format!("fn main() {{\n{}\n}}", source);
            match analyze_program(&wrapped) {
                Analysis::Analyzed(program) => run_body(source, program),
                Analysis::Failed(error) => Err(RecordError::new(error.line, error.detail)),
            }
        }
    }
}

fn run(source: &str, program: ProgramTree) -> Result<Recording, RecordError> {
    let units = parse_units(source);
    let lines: Vec<String> = source.lines().map(|line| line.to_string()).collect();
    let main = match program.function_named("main") {
        Some(main) => main,
        None => return Err(RecordError::new(1, "the program needs a main function")),
    };
    let events = {
        let mut engine = Engine::new(&program, units, lines);
        engine.call(main, Vec::new(), 1)?;
        engine.events
    };
    Ok(Recording { program, events })
}

fn run_body(source: &str, program: ProgramTree) -> Result<Recording, RecordError> {
    let main = match program.function_named("main") {
        Some(main) => main,
        None => return Err(RecordError::new(1, "the program needs a main function")),
    };
    let lines: Vec<String> = source.lines().map(|line| line.to_string()).collect();
    let unit = Unit {
        name: "main".to_string(),
        body_start: 1,
        body_end: lines.len() as u32,
    };
    let events = {
        let mut engine = Engine::new(&program, vec![unit], lines);
        engine.call(main, Vec::new(), 1)?;
        engine.events
    };
    Ok(Recording { program, events })
}

// ------------------------------------------------------------------- units

/// A function body located in the source, so calls can run it.
#[derive(Clone, Debug)]
struct Unit {
    name: String,
    body_start: u32,
    body_end: u32,
}

fn parse_units(source: &str) -> Vec<Unit> {
    let lines: Vec<&str> = source.lines().collect();
    let mut units = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let text = lines[index].trim();
        let name = match text.strip_prefix("fn ").and_then(function_name) {
            Some(name) => name,
            None => {
                index += 1;
                continue;
            }
        };
        let close = block_close(&lines, index as u32 + 1).unwrap_or(lines.len() as u32);
        units.push(Unit {
            name,
            body_start: index as u32 + 2,
            body_end: close.saturating_sub(1),
        });
        index = close as usize;
    }
    units
}

fn function_name(text: &str) -> Option<String> {
    let open = text.find('(')?;
    let name = text[..open].trim();
    match name.is_empty() {
        true => None,
        false => Some(name.to_string()),
    }
}

/// Line where the first block opened in `header` closes. Braces inside string
/// and character literals are ignored. A line such as `} else {` closes the
/// first block and opens the next one, so the scan stops at the `}` and the
/// caller can ask again from the same line for the `else` body.
fn block_close(lines: &[&str], header: u32) -> Option<u32> {
    let mut depth = 0;
    let mut started = false;
    for line in header..=lines.len() as u32 {
        let text = lines[(line - 1) as usize];
        let mut quote: Option<char> = None;
        let mut escaped = false;
        for character in text.chars() {
            if let Some(open) = quote {
                if escaped {
                    escaped = false;
                } else if character == '\\' {
                    escaped = true;
                } else if character == open {
                    quote = None;
                }
                continue;
            }
            match character {
                '"' | '\'' => quote = Some(character),
                '{' if !started => {
                    depth = 1;
                    started = true;
                }
                '{' => depth += 1,
                '}' if started => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(line);
                    }
                }
                _ => {}
            }
        }
    }
    None
}

// ------------------------------------------------------------------ engine

/// What running a statement did: continue at a line, or leave the function.
enum Flow {
    Next(u32),
    Return(PrimitiveValue),
}

#[derive(Clone, Debug)]
struct Local {
    value: PrimitiveValue,
    mutable: bool,
}

#[derive(Clone, Debug)]
struct Frame {
    function: FunctionId,
    id: FrameId,
    locals: HashMap<String, Local>,
    scopes: Vec<ScopeId>,
    names: Vec<Vec<String>>,
}

struct Engine<'a> {
    program: &'a ProgramTree,
    units: Vec<Unit>,
    lines: Vec<String>,
    events: Vec<TraceEvent>,
    sequence: u64,
    next_scope: u32,
    next_frame: u32,
    frames: Vec<Frame>,
}

impl<'a> Engine<'a> {
    fn new(program: &'a ProgramTree, units: Vec<Unit>, lines: Vec<String>) -> Self {
        Engine {
            program,
            units,
            lines,
            events: Vec::new(),
            sequence: 0,
            next_scope: 1,
            next_frame: 1,
            frames: Vec::new(),
        }
    }

    fn call(
        &mut self,
        function: FunctionId,
        arguments: Vec<PrimitiveValue>,
        call_line: u32,
    ) -> Result<PrimitiveValue, RecordError> {
        if self.frames.len() >= MAX_DEPTH {
            return Err(RecordError::new(
                call_line,
                "the call stack went too deep; check for a recursive call",
            ));
        }
        let (name, start_line, parameters) = match self.program.function(function) {
            Some(definition) => (
                definition.name.clone(),
                definition.start_line,
                definition.parameters.clone(),
            ),
            None => {
                return Err(RecordError::new(call_line, "unknown function"));
            }
        };
        if arguments.len() != parameters.len() {
            return Err(RecordError::new(
                call_line,
                format!(
                    "`{}` takes {} argument(s), got {}",
                    name,
                    parameters.len(),
                    arguments.len()
                ),
            ));
        }
        let unit = match self.units.iter().find(|unit| unit.name == name) {
            Some(unit) => unit.clone(),
            None => {
                return Err(RecordError::new(
                    call_line,
                    format!("the body of `{}` was not found", name),
                ));
            }
        };
        let mut fields = Vec::new();
        for (index, declaration) in parameters.iter().enumerate() {
            let value = fit(&declaration.data_type, arguments[index].clone()).ok_or_else(|| {
                RecordError::new(
                    call_line,
                    format!("argument `{}` has the wrong type", declaration.name),
                )
            })?;
            fields.push(Field {
                name: declaration.name.clone(),
                value: Value::Primitive(value),
            });
        }
        let mut frame = Frame {
            function,
            id: FrameId(self.next_frame),
            locals: HashMap::new(),
            scopes: Vec::new(),
            names: Vec::new(),
        };
        self.next_frame += 1;
        for field in &fields {
            if let Value::Primitive(value) = &field.value {
                frame.locals.insert(
                    field.name.clone(),
                    Local {
                        value: value.clone(),
                        mutable: true,
                    },
                );
            }
        }
        self.frames.push(frame);
        let sequence = self.next_sequence();
        self.events.push(TraceEvent::StartFunction {
            sequence,
            function,
            call_line,
            arguments: fields,
        });
        self.enter_scope(start_line, ScopeKind::Function);
        let returned = match self.run_range(unit.body_start, unit.body_end) {
            Ok(flow) => flow,
            Err(error) => {
                self.leave_function(None);
                return Err(error);
            }
        };
        let value = returned.clone().unwrap_or(PrimitiveValue::Integer(0));
        self.leave_function(returned);
        Ok(value)
    }

    fn leave_function(&mut self, returned: Option<PrimitiveValue>) {
        while self
            .frames
            .last()
            .is_some_and(|frame| !frame.scopes.is_empty())
        {
            self.exit_scope();
        }
        let sequence = self.next_sequence();
        self.events.push(TraceEvent::EndFunction {
            sequence,
            return_value: returned.map(Value::Primitive),
        });
        self.frames.pop();
    }

    fn run_range(&mut self, start: u32, end: u32) -> Result<Option<PrimitiveValue>, RecordError> {
        let mut line = start;
        while line <= end {
            let text = self.text(line).to_string();
            if text.is_empty() || text.starts_with("//") {
                line += 1;
                continue;
            }
            if text == "{" {
                self.enter_scope(line, ScopeKind::Block);
                line += 1;
                continue;
            }
            if text == "}" {
                self.exit_scope();
                line += 1;
                continue;
            }
            if text.starts_with("if ") {
                match self.run_if(line)? {
                    Flow::Next(next) => {
                        line = next;
                        continue;
                    }
                    Flow::Return(value) => return Ok(Some(value)),
                }
            }
            if text.starts_with("return") {
                let value = self.returned(&text, line)?;
                return Ok(Some(value));
            }
            if let Some(rest) = text.strip_prefix("let ") {
                self.declare(line, rest)?;
                line += 1;
                continue;
            }
            if is_print(&text) {
                line += 1;
                continue;
            }
            self.assign(&text, line)?;
            line += 1;
        }
        Ok(None)
    }

    fn returned(&mut self, text: &str, line: u32) -> Result<PrimitiveValue, RecordError> {
        let expression = text.trim_start_matches("return").trim();
        if expression.is_empty() {
            return Ok(PrimitiveValue::Integer(0));
        }
        let expression = expression.trim_end_matches(';').trim();
        evaluate(expression, line, self)
    }

    fn declare(&mut self, line: u32, rest: &str) -> Result<(), RecordError> {
        let statement = rest.trim().trim_end_matches(';').trim();
        let (mutable, statement) = match statement.strip_prefix("mut ") {
            Some(tail) => (true, tail),
            None => (false, statement),
        };
        let (name, type_text) = statement
            .split_once(':')
            .ok_or_else(|| RecordError::new(line, "expected `let name: Type = value;`"))?;
        let name = name.trim();
        let (_, expression) = type_text
            .split_once('=')
            .ok_or_else(|| RecordError::new(line, "every binding needs an initializer"))?;
        let declared = self.declaration(name, line)?.clone();
        let value = evaluate(expression.trim(), line, self)?;
        let value = fit(&declared, value)
            .ok_or_else(|| RecordError::new(line, format!("`{}` cannot hold that value", name)))?;
        if self.frame().locals.contains_key(name) {
            return Err(RecordError::new(
                line,
                format!("`{}` is already declared in this frame", name),
            ));
        }
        let sequence = self.next_sequence();
        self.events.push(TraceEvent::DeclareVariable {
            sequence,
            line,
            name: name.to_string(),
            value: Value::Primitive(value.clone()),
        });
        let frame = self.frame_mut();
        frame
            .locals
            .insert(name.to_string(), Local { value, mutable });
        if let Some(names) = frame.names.last_mut() {
            names.push(name.to_string());
        }
        Ok(())
    }

    fn assign(&mut self, text: &str, line: u32) -> Result<(), RecordError> {
        let statement = text.trim().trim_end_matches(';').trim();
        let (name, operator, expression) = split_assignment(statement).ok_or_else(|| {
            RecordError::new(line, format!("unsupported statement `{}`", statement))
        })?;
        let current = self
            .binding(name)
            .cloned()
            .ok_or_else(|| RecordError::new(line, format!("`{}` is not declared here", name)))?;
        if !current.mutable {
            return Err(RecordError::new(
                line,
                format!("cannot assign to the immutable binding `{}`", name),
            ));
        }
        let right = evaluate(expression, line, self)?;
        let value = match operator {
            "=" => right,
            _ => apply_operator(&operator[..operator.len() - 1], &current.value, &right)
                .ok_or_else(|| RecordError::new(line, "the operator does not fit these values"))?,
        };
        let declared = self.declaration(name, line)?.clone();
        let value = fit(&declared, value)
            .ok_or_else(|| RecordError::new(line, format!("`{}` cannot hold that value", name)))?;
        let frame = self.frame().id;
        let path = Path::Stack {
            frame,
            variable: name.to_string(),
        };
        let sequence = self.next_sequence();
        self.events.push(TraceEvent::WriteMemory {
            sequence,
            line,
            path,
            value: Value::Primitive(value.clone()),
        });
        self.frame_mut().locals.insert(
            name.to_string(),
            Local {
                value,
                mutable: true,
            },
        );
        Ok(())
    }

    fn run_if(&mut self, line: u32) -> Result<Flow, RecordError> {
        let text = self.text(line).to_string();
        let condition = text
            .strip_prefix("if ")
            .and_then(|rest| rest.strip_suffix('{'))
            .ok_or_else(|| RecordError::new(line, "the `if` needs its `{` on the same line"))?
            .trim()
            .to_string();
        let close = block_close(&self.line_refs(), line)
            .ok_or_else(|| RecordError::new(line, "the `if` is never closed"))?;
        let has_else = self.has_else(close);
        if has_else && self.text(close).contains("else if") {
            return Err(RecordError::new(
                close,
                "`else if` is not supported; nest the condition inside `else { ... }`",
            ));
        }
        let else_close = match has_else {
            true => Some(
                block_close(&self.line_refs(), close)
                    .ok_or_else(|| RecordError::new(close, "the `else` block is never closed"))?,
            ),
            false => None,
        };
        let after = else_close.unwrap_or(close) + 1;
        let value = evaluate(&condition, line, self)?;
        let taken = match value {
            PrimitiveValue::Boolean(value) => value,
            _ => return Err(RecordError::new(line, "the condition must be a `bool`")),
        };
        let target = match (taken, has_else) {
            (true, _) => line + 1,
            (false, true) => close + 1,
            (false, false) => after,
        };
        let sequence = self.next_sequence();
        self.events.push(TraceEvent::ConditionalJump {
            sequence,
            from_line: line,
            to_line: target,
            expression: condition,
            outcome: if taken {
                ConditionOutcome::True
            } else {
                ConditionOutcome::False
            },
        });
        let (start, end) = match (taken, has_else, else_close) {
            (true, _, _) => (line + 1, close.saturating_sub(1)),
            (false, true, Some(else_close)) => (close + 1, else_close.saturating_sub(1)),
            (false, _, _) => return Ok(Flow::Next(after)),
        };
        let open_line = match taken {
            true => line,
            false => close,
        };
        self.enter_scope(open_line, ScopeKind::Block);
        let flow = self.run_range(start, end)?;
        self.exit_scope();
        match flow {
            Some(value) => Ok(Flow::Return(value)),
            None => Ok(Flow::Next(after)),
        }
    }

    fn has_else(&self, line: u32) -> bool {
        self.text(line).starts_with("} else")
    }

    fn enter_scope(&mut self, line: u32, kind: ScopeKind) {
        let scope = ScopeId(self.next_scope);
        self.next_scope += 1;
        let parent = self.frame().scopes.last().copied();
        let frame = self.frame_mut();
        frame.scopes.push(scope);
        frame.names.push(Vec::new());
        let sequence = self.next_sequence();
        self.events.push(TraceEvent::EnterScope {
            sequence,
            line,
            scope,
            kind,
            parent,
        });
    }

    fn exit_scope(&mut self) {
        let frame = self.frame_mut();
        let scope = match frame.scopes.pop() {
            Some(scope) => scope,
            None => return,
        };
        if let Some(names) = frame.names.pop() {
            for name in names {
                frame.locals.remove(&name);
            }
        }
        let sequence = self.next_sequence();
        self.events.push(TraceEvent::ExitScope { sequence, scope });
    }

    fn text(&self, line: u32) -> &str {
        self.lines
            .get((line - 1) as usize)
            .map(|text| text.trim())
            .unwrap_or("")
    }

    fn line_refs(&self) -> Vec<&str> {
        self.lines.iter().map(|line| line.as_str()).collect()
    }

    fn local(&self, name: &str) -> Option<&PrimitiveValue> {
        self.frames
            .last()
            .and_then(|frame| frame.locals.get(name))
            .map(|local| &local.value)
    }

    fn binding(&self, name: &str) -> Option<&Local> {
        self.frames.last().and_then(|frame| frame.locals.get(name))
    }

    fn declaration(&self, name: &str, line: u32) -> Result<&DataType, RecordError> {
        let function = self
            .frames
            .last()
            .map(|frame| frame.function)
            .ok_or_else(|| RecordError::new(line, "no frame is running"))?;
        self.program
            .find_declaration(function, name)
            .map(|declaration| &declaration.data_type)
            .ok_or_else(|| RecordError::new(line, format!("unknown declaration `{}`", name)))
    }

    fn frame(&self) -> &Frame {
        self.frames.last().expect("a call always pushes a frame")
    }

    fn frame_mut(&mut self) -> &mut Frame {
        self.frames
            .last_mut()
            .expect("a call always pushes a frame")
    }

    fn next_sequence(&mut self) -> Sequence {
        self.sequence += 1;
        Sequence(self.sequence)
    }
}

fn is_print(text: &str) -> bool {
    ["println!", "print!", "eprintln!", "eprint!", "dbg!"]
        .iter()
        .any(|macro_name| text.starts_with(macro_name))
}

fn split_assignment(text: &str) -> Option<(&str, &str, &str)> {
    for operator in ["+=", "-=", "*=", "/=", "%=", "="] {
        let index = match text.find(operator) {
            Some(index) => index,
            None => continue,
        };
        let name = text[..index].trim();
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            continue;
        }
        if operator == "=" && text[index..].starts_with("==") {
            continue;
        }
        return Some((name, operator, text[index + operator.len()..].trim()));
    }
    None
}

// -------------------------------------------------------------- expressions

/// Parses and evaluates one expression against the running engine.
fn evaluate(text: &str, line: u32, engine: &mut Engine<'_>) -> Result<PrimitiveValue, RecordError> {
    let tokens = tokenize(text).ok_or_else(|| RecordError::new(line, "unreadable expression"))?;
    let mut parser = Parser {
        tokens,
        at: 0,
        line,
        engine,
    };
    let value = parser.or()?;
    match parser.at == parser.tokens.len() {
        true => Ok(value),
        false => Err(RecordError::new(
            line,
            "unexpected text after the expression",
        )),
    }
}

#[derive(Clone, PartialEq, Debug)]
enum Token {
    Number(String),
    Text(String),
    Char(char),
    Name(String),
    Op(&'static str),
    Open,
    Close,
    Comma,
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
            '"' => tokens.push(Token::Text(read_quoted(&mut chars, '"')?)),
            '\'' => tokens.push(Token::Char(read_character(&mut chars)?)),
            '+' => tokens.push(Token::Op("+")),
            '-' => tokens.push(Token::Op("-")),
            '*' => tokens.push(Token::Op("*")),
            '/' => tokens.push(Token::Op("/")),
            '%' => tokens.push(Token::Op("%")),
            '(' => tokens.push(Token::Open),
            ')' => tokens.push(Token::Close),
            ',' => tokens.push(Token::Comma),
            '!' => match chars.peek() {
                Some('=') => {
                    chars.next();
                    tokens.push(Token::Op("!="));
                }
                _ => tokens.push(Token::Op("!")),
            },
            '=' => match chars.peek() {
                Some('=') => {
                    chars.next();
                    tokens.push(Token::Op("=="));
                }
                _ => return None,
            },
            '<' => match chars.peek() {
                Some('=') => {
                    chars.next();
                    tokens.push(Token::Op("<="));
                }
                _ => tokens.push(Token::Op("<")),
            },
            '>' => match chars.peek() {
                Some('=') => {
                    chars.next();
                    tokens.push(Token::Op(">="));
                }
                _ => tokens.push(Token::Op(">")),
            },
            '&' => match chars.peek() {
                Some('&') => {
                    chars.next();
                    tokens.push(Token::Op("&&"));
                }
                _ => return None,
            },
            '|' => match chars.peek() {
                Some('|') => {
                    chars.next();
                    tokens.push(Token::Op("||"));
                }
                _ => return None,
            },
            _ => return None,
        }
    }
    Some(tokens)
}

fn read_quoted(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, end: char) -> Option<String> {
    let mut text = String::new();
    let mut escaped = false;
    for character in chars.by_ref() {
        if escaped {
            text.push(match character {
                'n' => '\n',
                't' => '\t',
                other => other,
            });
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            value if value == end => return Some(text),
            other => text.push(other),
        }
    }
    None
}

fn read_character(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Option<char> {
    let text = read_quoted(chars, '\'')?;
    let mut characters = text.chars();
    match (characters.next(), characters.next()) {
        (Some(value), None) => Some(value),
        _ => None,
    }
}

struct Parser<'a, 'b> {
    tokens: Vec<Token>,
    at: usize,
    line: u32,
    engine: &'a mut Engine<'b>,
}

impl Parser<'_, '_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.at).cloned()?;
        self.at += 1;
        Some(token)
    }

    fn or(&mut self) -> Result<PrimitiveValue, RecordError> {
        let mut value = self.and()?;
        while self.peek() == Some(&Token::Op("||")) {
            self.at += 1;
            let right = self.and()?;
            value =
                PrimitiveValue::Boolean(as_bool(&value, self.line)? || as_bool(&right, self.line)?);
        }
        Ok(value)
    }

    fn and(&mut self) -> Result<PrimitiveValue, RecordError> {
        let mut value = self.comparison()?;
        while self.peek() == Some(&Token::Op("&&")) {
            self.at += 1;
            let right = self.comparison()?;
            value =
                PrimitiveValue::Boolean(as_bool(&value, self.line)? && as_bool(&right, self.line)?);
        }
        Ok(value)
    }

    fn comparison(&mut self) -> Result<PrimitiveValue, RecordError> {
        let mut value = self.sum()?;
        while let Some(operator) = comparison_operator(self.peek()) {
            self.at += 1;
            let right = self.sum()?;
            let outcome = compare(operator, &value, &right).ok_or_else(|| {
                RecordError::new(
                    self.line,
                    "comparison between values that cannot be compared",
                )
            })?;
            value = PrimitiveValue::Boolean(outcome);
        }
        Ok(value)
    }

    fn sum(&mut self) -> Result<PrimitiveValue, RecordError> {
        let mut value = self.term()?;
        loop {
            let operator = match self.peek() {
                Some(Token::Op("+")) => "+",
                Some(Token::Op("-")) => "-",
                _ => return Ok(value),
            };
            self.at += 1;
            let right = self.term()?;
            value = apply_operator(operator, &value, &right).ok_or_else(|| {
                RecordError::new(self.line, "arithmetic between incompatible values")
            })?;
        }
    }

    fn term(&mut self) -> Result<PrimitiveValue, RecordError> {
        let mut value = self.factor()?;
        loop {
            let operator = match self.peek() {
                Some(Token::Op("*")) => "*",
                Some(Token::Op("/")) => "/",
                Some(Token::Op("%")) => "%",
                _ => return Ok(value),
            };
            self.at += 1;
            let right = self.factor()?;
            value = apply_operator(operator, &value, &right).ok_or_else(|| {
                RecordError::new(self.line, "arithmetic between incompatible values")
            })?;
        }
    }

    fn factor(&mut self) -> Result<PrimitiveValue, RecordError> {
        match self.bump() {
            Some(Token::Op("-")) => {
                let value = self.factor()?;
                negate(&value)
                    .ok_or_else(|| RecordError::new(self.line, "cannot negate this value"))
            }
            Some(Token::Op("!")) => {
                let value = self.factor()?;
                Ok(PrimitiveValue::Boolean(!as_bool(&value, self.line)?))
            }
            Some(Token::Open) => {
                let value = self.or()?;
                match self.bump() {
                    Some(Token::Close) => Ok(value),
                    _ => Err(RecordError::new(self.line, "missing `)`")),
                }
            }
            Some(Token::Number(text)) => match text.contains('.') {
                true => text
                    .parse()
                    .map(PrimitiveValue::Float)
                    .map_err(|_| RecordError::new(self.line, "unreadable number")),
                false => text
                    .parse()
                    .map(PrimitiveValue::Integer)
                    .map_err(|_| RecordError::new(self.line, "unreadable number")),
            },
            Some(Token::Text(text)) => Ok(PrimitiveValue::Text(text)),
            Some(Token::Char(value)) => Ok(PrimitiveValue::Character(value)),
            Some(Token::Name(name)) => self.name(name),
            _ => Err(RecordError::new(self.line, "incomplete expression")),
        }
    }

    fn name(&mut self, name: String) -> Result<PrimitiveValue, RecordError> {
        match name.as_str() {
            "true" => return Ok(PrimitiveValue::Boolean(true)),
            "false" => return Ok(PrimitiveValue::Boolean(false)),
            _ => {}
        }
        if self.peek() == Some(&Token::Open) {
            return self.call(name);
        }
        self.engine
            .local(&name)
            .cloned()
            .ok_or_else(|| RecordError::new(self.line, format!("`{}` is not declared here", name)))
    }

    fn call(&mut self, name: String) -> Result<PrimitiveValue, RecordError> {
        self.bump();
        let mut arguments = Vec::new();
        if self.peek() == Some(&Token::Close) {
            self.bump();
        } else {
            loop {
                arguments.push(self.or()?);
                match self.bump() {
                    Some(Token::Comma) => continue,
                    Some(Token::Close) => break,
                    _ => return Err(RecordError::new(self.line, "missing `)` in the call")),
                }
            }
        }
        let function =
            self.engine.program.function_named(&name).ok_or_else(|| {
                RecordError::new(self.line, format!("unknown function `{}`", name))
            })?;
        let call_line = self.line;
        self.engine.call(function, arguments, call_line)
    }
}

fn comparison_operator(token: Option<&Token>) -> Option<&'static str> {
    match token {
        Some(Token::Op("==")) => Some("=="),
        Some(Token::Op("!=")) => Some("!="),
        Some(Token::Op("<")) => Some("<"),
        Some(Token::Op(">")) => Some(">"),
        Some(Token::Op("<=")) => Some("<="),
        Some(Token::Op(">=")) => Some(">="),
        _ => None,
    }
}

fn compare(operator: &str, left: &PrimitiveValue, right: &PrimitiveValue) -> Option<bool> {
    if let (PrimitiveValue::Text(left), PrimitiveValue::Text(right)) = (left, right) {
        return match operator {
            "==" => Some(left == right),
            "!=" => Some(left != right),
            _ => None,
        };
    }
    let left = numeric(left)?;
    let right = numeric(right)?;
    Some(match operator {
        "==" => left == right,
        "!=" => left != right,
        "<" => left < right,
        ">" => left > right,
        "<=" => left <= right,
        ">=" => left >= right,
        _ => return None,
    })
}

fn numeric(value: &PrimitiveValue) -> Option<f64> {
    match value {
        PrimitiveValue::Integer(value) => Some(*value as f64),
        PrimitiveValue::Float(value) => Some(*value),
        _ => None,
    }
}

fn both_integers(left: &PrimitiveValue, right: &PrimitiveValue) -> bool {
    matches!(left, PrimitiveValue::Integer(_)) && matches!(right, PrimitiveValue::Integer(_))
}

fn apply_operator(
    operator: &str,
    left: &PrimitiveValue,
    right: &PrimitiveValue,
) -> Option<PrimitiveValue> {
    if let (PrimitiveValue::Text(left), PrimitiveValue::Text(right)) = (left, right) {
        return match operator {
            "+" => Some(PrimitiveValue::Text(format!("{}{}", left, right))),
            _ => None,
        };
    }
    if both_integers(left, right) {
        let (PrimitiveValue::Integer(left), PrimitiveValue::Integer(right)) = (left, right) else {
            return None;
        };
        let value = match operator {
            "+" => left.checked_add(*right)?,
            "-" => left.checked_sub(*right)?,
            "*" => left.checked_mul(*right)?,
            "/" => left.checked_div(*right)?,
            "%" => left.checked_rem(*right)?,
            _ => return None,
        };
        return Some(PrimitiveValue::Integer(value));
    }
    let left = numeric(left)?;
    let right = numeric(right)?;
    let value = match operator {
        "+" => left + right,
        "-" => left - right,
        "*" => left * right,
        "/" => left / right,
        "%" => left % right,
        _ => return None,
    };
    Some(PrimitiveValue::Float(value))
}

fn negate(value: &PrimitiveValue) -> Option<PrimitiveValue> {
    match value {
        PrimitiveValue::Integer(value) => value.checked_neg().map(PrimitiveValue::Integer),
        PrimitiveValue::Float(value) => Some(PrimitiveValue::Float(-value)),
        _ => None,
    }
}

fn as_bool(value: &PrimitiveValue, line: u32) -> Result<bool, RecordError> {
    match value {
        PrimitiveValue::Boolean(value) => Ok(*value),
        _ => Err(RecordError::new(line, "the condition must be a `bool`")),
    }
}

/// Narrows a value to the declared type, widening integers to floats.
fn fit(declared: &DataType, value: PrimitiveValue) -> Option<PrimitiveValue> {
    match (declared, value) {
        (DataType::Integer, PrimitiveValue::Integer(value)) => Some(PrimitiveValue::Integer(value)),
        (DataType::Float, PrimitiveValue::Integer(value)) => {
            Some(PrimitiveValue::Float(value as f64))
        }
        (DataType::Float, PrimitiveValue::Float(value)) => Some(PrimitiveValue::Float(value)),
        (DataType::Boolean, PrimitiveValue::Boolean(value)) => Some(PrimitiveValue::Boolean(value)),
        (DataType::Character, PrimitiveValue::Character(value)) => {
            Some(PrimitiveValue::Character(value))
        }
        (DataType::Text, PrimitiveValue::Text(value)) => Some(PrimitiveValue::Text(value)),
        (DataType::Structure(_), _) | (DataType::Sequence(_), _) => None,
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::memory::MemoryState;
    use crate::stages::replay::{Timeline, build_timeline};

    fn recorded(source: &str) -> Recording {
        record(source).unwrap_or_else(|error| panic!("line {}: {}", error.line, error.detail))
    }

    fn timeline(recording: &Recording) -> Timeline {
        build_timeline(&recording.program, &recording.events)
    }

    /// Step right after the last event that writes `name`. Reading the end of
    /// the trace would see the frame already popped, or its variables dropped
    /// by `ExitScope`; a binding is observable between its write and its drop.
    fn state_after(source: &str, name: &str) -> MemoryState {
        let recording = recorded(source);
        let timeline = timeline(&recording);
        let step = recording
            .events
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, event)| match event {
                TraceEvent::DeclareVariable { name: written, .. } if written == name => {
                    Some(index + 1)
                }
                TraceEvent::WriteMemory {
                    path: Path::Stack { variable, .. },
                    ..
                } if variable == name => Some(index + 1),
                _ => None,
            })
            .unwrap_or_else(|| panic!("`{}` is never written", name));
        timeline.state_at(step).expect("the step exists").clone()
    }

    fn stack_value(state: &MemoryState, name: &str) -> PrimitiveValue {
        state
            .frames
            .last()
            .and_then(|frame| {
                frame
                    .variables
                    .iter()
                    .find(|variable| variable.name == name)
            })
            .map(|variable| match &variable.value {
                Value::Primitive(value) => value.clone(),
                other => panic!("{} is not primitive: {:?}", name, other),
            })
            .unwrap_or_else(|| panic!("{} is not in the frame", name))
    }

    #[test]
    fn a_binding_shows_its_value_in_the_frame() {
        let state = state_after("fn main() {\n let a: i32 = 2 + 3 * 4;\n}", "a");
        assert_eq!(stack_value(&state, "a"), PrimitiveValue::Integer(14));
    }

    #[test]
    fn a_mutation_writes_the_new_value() {
        let source = "fn main() {\n let mut a: i32 = 1;\n a = a + 2;\n a += 3;\n}";
        let recording = recorded(source);
        let state = state_after(source, "a");
        assert_eq!(stack_value(&state, "a"), PrimitiveValue::Integer(6));
        let writes = recording
            .events
            .iter()
            .filter(|event| matches!(event, TraceEvent::WriteMemory { .. }))
            .count();
        assert_eq!(writes, 2, "both assignments reach the tape");
    }

    #[test]
    fn an_immutable_binding_refuses_the_write() {
        let error = record("fn main() {\n let a: i32 = 1;\n a = 2;\n}")
            .expect_err("the write must be rejected");
        assert_eq!(error.line, 3);
        assert!(error.detail.contains("immutable"));
    }

    #[test]
    fn a_call_pushes_a_frame_and_returns_its_value() {
        let source = "fn add(a: i32, b: i32) -> i32 {\n return a + b;\n}\n\nfn main() {\n let total: i32 = add(2, 3);\n}";
        let recording = recorded(source);
        let starts = recording
            .events
            .iter()
            .filter(|event| matches!(event, TraceEvent::StartFunction { .. }))
            .count();
        assert_eq!(starts, 2, "main and add");
        let state = state_after(source, "total");
        assert_eq!(state.frames.len(), 1, "add's frame was popped");
        assert_eq!(stack_value(&state, "total"), PrimitiveValue::Integer(5));
    }

    #[test]
    fn a_condition_chooses_one_branch() {
        let source = "fn main() {\n let limit: i32 = 4;\n if limit > 2 {\n let big: i32 = 1;\n } else {\n let small: i32 = 2;\n }\n}";
        let recording = recorded(source);
        let state = state_after(source, "big");
        let names: Vec<&str> = state
            .frames
            .last()
            .map(|frame| frame.variables.iter().map(|v| v.name.as_str()).collect())
            .unwrap_or_default();
        assert!(names.contains(&"big"), "the taken branch declares big");
        assert!(!names.contains(&"small"), "the other branch never runs");
        assert!(recording.events.iter().any(|event| matches!(
            event,
            TraceEvent::ConditionalJump {
                outcome: ConditionOutcome::True,
                ..
            }
        )));
    }

    #[test]
    fn a_block_closes_its_bindings() {
        let recording = recorded("fn main() {\n let a: i32 = 1;\n {\n let b: i32 = a + 1;\n }\n}");
        let scopes: Vec<ScopeId> = recording
            .events
            .iter()
            .filter_map(|event| match event {
                TraceEvent::ExitScope { scope, .. } => Some(*scope),
                _ => None,
            })
            .collect();
        assert_eq!(scopes, vec![ScopeId(2), ScopeId(1)]);
    }

    #[test]
    fn a_bare_sequence_of_statements_records_without_a_main() {
        let recording = recorded("let a: i32 = 1;\nlet b: i32 = a + 2;");
        assert!(matches!(
            recording.events.first(),
            Some(TraceEvent::StartFunction { .. })
        ));
        let state = state_after("let a: i32 = 1;\nlet b: i32 = a + 2;", "b");
        assert_eq!(stack_value(&state, "b"), PrimitiveValue::Integer(3));
    }

    #[test]
    fn main_may_come_after_other_functions() {
        let source = "fn helper(x: i32) -> i32 {\n let y: i32 = x;\n return y;\n}\nfn main() {\n let a: i32 = 2;\n}";
        let recording = recorded(source);
        assert!(
            !recording.events.iter().any(
                |event| matches!(event, TraceEvent::DeclareVariable { name, .. } if name == "y")
            )
        );
        let state = state_after(source, "a");
        assert_eq!(stack_value(&state, "a"), PrimitiveValue::Integer(2));
    }

    #[test]
    fn a_print_macro_does_not_block_the_trace() {
        let source = "fn main() {\n let a: i32 = 1;\n println!(\"a = {}\", a);\n}";
        let state = state_after(source, "a");
        assert_eq!(stack_value(&state, "a"), PrimitiveValue::Integer(1));
    }

    #[test]
    fn an_unsupported_statement_reports_its_line() {
        let error = record("fn main() {\n let a: i32 = 1;\n a.foo();\n}")
            .expect_err("method calls are not modelled");
        assert_eq!(error.line, 3);
    }

    #[test]
    fn the_lifecycle_scene_records_and_evaluates_arithmetic() {
        let recording = recorded(crate::edge::sample::LIFECYCLE_SOURCE);
        assert!(recording.events.iter().any(
            |event| matches!(event, TraceEvent::DeclareVariable { name, value, .. }
                if name == "result" && *value == Value::Primitive(PrimitiveValue::Integer(8)))
        ));
    }
}
