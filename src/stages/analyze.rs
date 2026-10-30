use crate::model::ids::{FunctionId, StructId};
use crate::model::program::{
    CallSite, DataType, Declaration, FieldDef, FunctionDef, ProgramTree, StructDef,
};

const ENTRY_NAME: &str = "main";

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SyntaxError {
    pub line: u32,
    pub detail: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Analysis {
    Analyzed(ProgramTree),
    Failed(SyntaxError),
}

pub fn analyze_program(source: &str) -> Analysis {
    let raw = match scan(source) {
        Ok(raw) => raw,
        Err(error) => return Analysis::Failed(error),
    };
    match build(&raw) {
        Ok(tree) => Analysis::Analyzed(tree),
        Err(error) => Analysis::Failed(error),
    }
}

// ---------------------------------------------------------------- raw program

#[derive(Clone, Debug)]
struct RawField {
    line: u32,
    name: String,
    type_text: String,
}

#[derive(Clone, Debug)]
struct RawCall {
    line: u32,
    name: String,
}

#[derive(Clone, Debug)]
struct RawStruct {
    name: String,
    line: u32,
    fields: Vec<RawField>,
}

#[derive(Clone, Debug)]
struct RawFunction {
    name: String,
    line: u32,
    parameters: Vec<RawField>,
    declarations: Vec<RawField>,
    calls: Vec<RawCall>,
}

#[derive(Clone, Debug, Default)]
struct RawProgram {
    structs: Vec<RawStruct>,
    functions: Vec<RawFunction>,
}

// -------------------------------------------------------------------- scanner

#[derive(Clone, Copy, Debug)]
enum Block {
    Structure(usize),
    Function(usize),
}

struct Scanner {
    program: RawProgram,
    block: Option<Block>,
    depth: i32,
}

fn scan(source: &str) -> Result<RawProgram, SyntaxError> {
    let mut scanner = Scanner {
        program: RawProgram::default(),
        block: None,
        depth: 0,
    };
    let mut result = Ok(());
    let mut index = 0;
    let lines: Vec<&str> = source.lines().collect();
    loop {
        if index >= lines.len() || result.is_err() {
            break;
        }
        let line = index as u32 + 1;
        result = scanner.read_line(line, lines[index]);
        index += 1;
    }
    match result {
        Ok(()) => scanner.finish(),
        Err(error) => Err(error),
    }
}

impl Scanner {
    fn read_line(&mut self, line: u32, raw: &str) -> Result<(), SyntaxError> {
        let text = raw.trim();
        let skip = text.is_empty() || text.starts_with("//");
        match skip {
            true => Ok(()),
            false => self.read_content(line, text),
        }
    }

    fn read_content(&mut self, line: u32, text: &str) -> Result<(), SyntaxError> {
        let opened = text.matches('{').count() as i32;
        let closed = text.matches('}').count() as i32;
        self.depth += opened - closed;
        match self.block {
            None => self.open_block(line, text),
            Some(Block::Structure(index)) => self.read_field(line, text, index),
            Some(Block::Function(index)) => self.read_statement(line, text, index),
        }
    }

    fn open_block(&mut self, line: u32, text: &str) -> Result<(), SyntaxError> {
        let structure_name = parse_struct_header(text);
        let function_header = match structure_name {
            Some(_) => None,
            None => parse_function_header(text),
        };
        match (structure_name, function_header) {
            (Some(name), _) => self.start_structure(line, name),
            (None, Some((name, parameters))) => self.start_function(line, name, &parameters),
            (None, None) => Ok(()),
        }
    }

    fn start_structure(&mut self, line: u32, name: String) -> Result<(), SyntaxError> {
        let duplicate = self
            .program
            .structs
            .iter()
            .find(|structure| structure.name == name);
        match duplicate {
            Some(existing) => Err(SyntaxError {
                line,
                detail: format!(
                    "duplicate struct '{}' (first defined at line {})",
                    name, existing.line
                ),
            }),
            None => {
                self.program.structs.push(RawStruct {
                    name,
                    line,
                    fields: Vec::new(),
                });
                self.block = Some(Block::Structure(self.program.structs.len() - 1));
                self.depth = 1;
                Ok(())
            }
        }
    }

    fn start_function(&mut self, line: u32, name: String, text: &str) -> Result<(), SyntaxError> {
        let duplicate = self
            .program
            .functions
            .iter()
            .find(|function| function.name == name);
        match duplicate {
            Some(existing) => Err(SyntaxError {
                line,
                detail: format!(
                    "duplicate function '{}' (first defined at line {})",
                    name, existing.line
                ),
            }),
            None => self.push_function(line, name, text),
        }
    }

    fn push_function(&mut self, line: u32, name: String, text: &str) -> Result<(), SyntaxError> {
        let parameters = match parse_parameters(text, line) {
            Ok(fields) => fields,
            Err(error) => return Err(error),
        };
        self.program.functions.push(RawFunction {
            name,
            line,
            parameters,
            declarations: Vec::new(),
            calls: Vec::new(),
        });
        self.block = Some(Block::Function(self.program.functions.len() - 1));
        self.depth = 1;
        Ok(())
    }

    fn read_field(&mut self, line: u32, text: &str, index: usize) -> Result<(), SyntaxError> {
        match self.depth <= 0 {
            true => self.close_block(),
            false => self.push_field(line, text, index),
        }
    }

    fn push_field(&mut self, line: u32, text: &str, index: usize) -> Result<(), SyntaxError> {
        match parse_field(text) {
            Some((name, type_text)) => {
                self.program.structs[index].fields.push(RawField {
                    line,
                    name,
                    type_text,
                });
                Ok(())
            }
            None => Err(SyntaxError {
                line,
                detail: format!(
                    "struct '{}': expected 'field: Type'",
                    self.program.structs[index].name
                ),
            }),
        }
    }

    fn read_statement(&mut self, line: u32, text: &str, index: usize) -> Result<(), SyntaxError> {
        match self.depth <= 0 {
            true => self.close_block(),
            false => {
                self.push_declaration(line, text, index);
                self.push_calls(line, text, index);
                Ok(())
            }
        }
    }

    fn push_declaration(&mut self, line: u32, text: &str, index: usize) {
        if let Some((name, type_text)) = parse_declaration(text) {
            self.program.functions[index].declarations.push(RawField {
                line,
                name,
                type_text,
            });
        }
    }

    fn push_calls(&mut self, line: u32, text: &str, index: usize) {
        for name in call_names(text) {
            self.program.functions[index]
                .calls
                .push(RawCall { line, name });
        }
    }

    fn close_block(&mut self) -> Result<(), SyntaxError> {
        self.block = None;
        self.depth = 0;
        Ok(())
    }

    fn finish(self) -> Result<RawProgram, SyntaxError> {
        let unterminated = match self.block {
            Some(Block::Structure(index)) => Some((
                self.program.structs[index].line,
                format!(
                    "struct '{}' is never closed",
                    self.program.structs[index].name
                ),
            )),
            Some(Block::Function(index)) => Some((
                self.program.functions[index].line,
                format!(
                    "function '{}' is never closed",
                    self.program.functions[index].name
                ),
            )),
            None => None,
        };
        match unterminated {
            Some((line, detail)) => Err(SyntaxError { line, detail }),
            None => Ok(self.program),
        }
    }
}

// ------------------------------------------------------------------ leaf text

fn parse_struct_header(text: &str) -> Option<String> {
    match text.strip_prefix("struct ") {
        Some(rest) => non_empty(rest.trim().trim_end_matches('{').trim()),
        None => None,
    }
}

fn parse_function_header(text: &str) -> Option<(String, String)> {
    match text.strip_prefix("fn ") {
        Some(rest) => split_signature(rest),
        None => None,
    }
}

fn split_signature(text: &str) -> Option<(String, String)> {
    match (text.find('('), text.find(')')) {
        (Some(open), Some(close)) if close > open => {
            let parameters = text[open + 1..close].trim().to_string();
            non_empty(text[..open].trim()).map(|name| (name, parameters))
        }
        _ => None,
    }
}

fn parse_field(text: &str) -> Option<(String, String)> {
    match text.split_once(':') {
        Some((name, type_text)) => clean_field(name, type_text),
        None => None,
    }
}

fn clean_field(name: &str, type_text: &str) -> Option<(String, String)> {
    let name = name.trim().trim_start_matches("pub ").trim().to_string();
    let without_default = match type_text.split_once('=') {
        Some((left, _)) => left,
        None => type_text,
    };
    let type_text = without_default
        .trim()
        .trim_end_matches(',')
        .trim()
        .to_string();
    match name.is_empty() || type_text.is_empty() {
        true => None,
        false => Some((name, type_text)),
    }
}

fn parse_declaration(text: &str) -> Option<(String, String)> {
    match text.strip_prefix("let ") {
        Some(rest) => parse_binding(rest),
        None => None,
    }
}

fn parse_binding(text: &str) -> Option<(String, String)> {
    let without_mut = match text.strip_prefix("mut ") {
        Some(tail) => tail,
        None => text,
    };
    parse_field(without_mut)
}

fn parse_parameters(text: &str, line: u32) -> Result<Vec<RawField>, SyntaxError> {
    let trimmed = text.trim();
    match trimmed.is_empty() {
        true => Ok(Vec::new()),
        false => collect_parameters(trimmed, line),
    }
}

fn collect_parameters(text: &str, line: u32) -> Result<Vec<RawField>, SyntaxError> {
    let mut parameters = Vec::new();
    let mut failure: Option<SyntaxError> = None;
    for item in text.split(',') {
        if failure.is_some() {
            break;
        }
        match parse_field(item) {
            Some((name, type_text)) => parameters.push(RawField {
                line,
                name,
                type_text,
            }),
            None => {
                failure = Some(SyntaxError {
                    line,
                    detail: format!("unreadable parameter '{}'", item.trim()),
                });
            }
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(parameters),
    }
}

fn call_names(text: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut current = String::new();
    for character in text.chars() {
        let is_name_char = character.is_ascii_alphanumeric() || character == '_';
        match is_name_char {
            true => current.push(character),
            false => {
                if character == '(' {
                    names.extend(non_empty(&current));
                }
                current.clear();
            }
        }
    }
    names
}

fn non_empty(text: &str) -> Option<String> {
    match text.is_empty() {
        true => None,
        false => Some(text.to_string()),
    }
}

// -------------------------------------------------------------------- builder

fn build(raw: &RawProgram) -> Result<ProgramTree, SyntaxError> {
    let struct_names: Vec<(String, StructId)> = raw
        .structs
        .iter()
        .enumerate()
        .map(|(index, structure)| (structure.name.clone(), StructId(index as u32)))
        .collect();
    let structs = build_structures(raw, &struct_names)?;
    let functions = build_functions(raw, &struct_names)?;
    let entry = find_entry(&functions)?;
    Ok(ProgramTree {
        entry,
        functions,
        structs,
    })
}

fn build_structures(
    raw: &RawProgram,
    struct_names: &[(String, StructId)],
) -> Result<Vec<StructDef>, SyntaxError> {
    let mut structs = Vec::new();
    let mut failure: Option<SyntaxError> = None;
    for (index, structure) in raw.structs.iter().enumerate() {
        if failure.is_some() {
            break;
        }
        let mut fields = Vec::new();
        for field in &structure.fields {
            match resolve_type(&field.type_text, struct_names, field.line) {
                Ok(data_type) => fields.push(FieldDef {
                    name: field.name.clone(),
                    data_type,
                }),
                Err(error) => failure = Some(error),
            }
        }
        structs.push(StructDef {
            id: StructId(index as u32),
            name: structure.name.clone(),
            fields,
        });
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(structs),
    }
}

fn build_functions(
    raw: &RawProgram,
    struct_names: &[(String, StructId)],
) -> Result<Vec<FunctionDef>, SyntaxError> {
    let names: Vec<(String, FunctionId)> = raw
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| (function.name.clone(), FunctionId(index as u32)))
        .collect();
    let mut functions = Vec::new();
    let mut failure: Option<SyntaxError> = None;
    for (index, function) in raw.functions.iter().enumerate() {
        if failure.is_some() {
            break;
        }
        let id = FunctionId(index as u32);
        match build_function(id, function, struct_names, &names) {
            Ok(definition) => functions.push(definition),
            Err(error) => failure = Some(error),
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(functions),
    }
}

fn build_function(
    id: FunctionId,
    raw: &RawFunction,
    struct_names: &[(String, StructId)],
    function_names: &[(String, FunctionId)],
) -> Result<FunctionDef, SyntaxError> {
    let mut parameters = Vec::new();
    let mut declarations = Vec::new();
    let mut calls = Vec::new();
    let mut failure: Option<SyntaxError> = None;
    for field in &raw.parameters {
        match build_declaration(field, struct_names) {
            Ok(declaration) => parameters.push(declaration),
            Err(error) => failure = Some(error),
        }
    }
    for field in &raw.declarations {
        match build_declaration(field, struct_names) {
            Ok(declaration) => declarations.push(declaration),
            Err(error) => failure = Some(error),
        }
    }
    for call in &raw.calls {
        if let Some(target) = find_function_id(&call.name, function_names) {
            calls.push(CallSite {
                target,
                line: call.line,
            });
        }
    }
    match failure {
        Some(error) => Err(error),
        None => Ok(FunctionDef {
            id,
            name: raw.name.clone(),
            start_line: raw.line,
            parameters,
            declarations,
            calls,
        }),
    }
}

fn build_declaration(
    field: &RawField,
    struct_names: &[(String, StructId)],
) -> Result<Declaration, SyntaxError> {
    let data_type = resolve_type(&field.type_text, struct_names, field.line)?;
    Ok(Declaration {
        name: field.name.clone(),
        data_type,
        line: field.line,
    })
}

fn find_function_id(name: &str, names: &[(String, FunctionId)]) -> Option<FunctionId> {
    names
        .iter()
        .find(|(candidate, _)| candidate == name)
        .map(|(_, id)| *id)
}

fn find_entry(functions: &[FunctionDef]) -> Result<FunctionId, SyntaxError> {
    let candidates: Vec<&FunctionDef> = functions
        .iter()
        .filter(|function| function.name == ENTRY_NAME)
        .collect();
    match candidates.as_slice() {
        [] => Err(SyntaxError {
            line: 1,
            detail: "missing 'main' function".to_string(),
        }),
        [only] => Ok(only.id),
        [first, second, ..] => Err(SyntaxError {
            line: second.start_line,
            detail: format!(
                "duplicate 'main' (first defined at line {})",
                first.start_line
            ),
        }),
    }
}

// ---------------------------------------------------------------------- types

fn resolve_type(
    text: &str,
    structures: &[(String, StructId)],
    line: u32,
) -> Result<DataType, SyntaxError> {
    let mut remaining = text.trim().trim_start_matches('&').trim().to_string();
    let mut wrappers = 0;
    loop {
        let inner = strip_sequence(&remaining);
        match inner {
            Some(inner) => {
                wrappers += 1;
                remaining = inner;
            }
            None => break,
        }
    }
    let base = resolve_base(&remaining, structures, line)?;
    let mut data_type = base;
    while wrappers > 0 {
        data_type = DataType::Sequence(Box::new(data_type));
        wrappers -= 1;
    }
    Ok(data_type)
}

/// Strips one `Vec<T>` or `[T; n]` layer and returns the inner type text.
fn strip_sequence(text: &str) -> Option<String> {
    let vec_inner = text
        .strip_prefix("Vec<")
        .map(|inner| inner.trim_end_matches('>'));
    match vec_inner {
        Some(inner) => non_empty(inner.trim()),
        None => strip_brackets(text),
    }
}

fn strip_brackets(text: &str) -> Option<String> {
    match text.strip_prefix('[') {
        Some(inner) => {
            let body = inner.trim_end_matches(']');
            let element = match body.split_once(';') {
                Some((left, _)) => left,
                None => body,
            };
            non_empty(element.trim())
        }
        None => None,
    }
}

fn resolve_base(
    text: &str,
    structures: &[(String, StructId)],
    line: u32,
) -> Result<DataType, SyntaxError> {
    match text {
        "i8" | "i16" | "i32" | "i64" | "i128" | "isize" | "u8" | "u16" | "u32" | "u64" | "u128"
        | "usize" => Ok(DataType::Integer),
        "f32" | "f64" => Ok(DataType::Float),
        "bool" => Ok(DataType::Boolean),
        "char" => Ok(DataType::Character),
        "String" | "str" => Ok(DataType::Text),
        other => match structures.iter().find(|(name, _)| name == other) {
            Some((_, id)) => Ok(DataType::Structure(*id)),
            None => Err(SyntaxError {
                line,
                detail: format!("unknown type '{}'", other),
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "\
struct Point {
    x: i32,
    y: i32,
}

fn build_point(x: i32, y: i32) -> Point {
    let result: Point = Point { x, y };
    return result;
}

fn main() {
    let origin: Point = build_point(0, 0);
    let scores: [i32; 3] = [90, 75, 88];
}
";

    fn tree(source: &str) -> ProgramTree {
        match analyze_program(source) {
            Analysis::Analyzed(tree) => tree,
            Analysis::Failed(error) => panic!("unexpected error: {:?}", error),
        }
    }

    #[test]
    fn reads_structs_functions_and_declarations() {
        let program = tree(SOURCE);
        assert_eq!(program.structs.len(), 1);
        assert_eq!(program.structs[0].fields.len(), 2);
        assert_eq!(program.functions.len(), 2);
        assert_eq!(program.function(program.entry).unwrap().name, "main");
    }

    #[test]
    fn reads_parameters_and_call_edges() {
        let program = tree(SOURCE);
        let main = program.function(program.entry).unwrap();
        assert_eq!(main.declarations.len(), 2);
        assert_eq!(main.calls.len(), 1);
        assert_eq!(
            program.function(main.calls[0].target).unwrap().name,
            "build_point"
        );
    }

    #[test]
    fn resolves_sequence_and_struct_types() {
        let program = tree(SOURCE);
        let main = program.function(program.entry).unwrap();
        let scores = main.declaration_named("scores").unwrap();
        match &scores.data_type {
            DataType::Sequence(inner) => assert_eq!(**inner, DataType::Integer),
            other => panic!("expected a sequence, got {:?}", other),
        }
    }

    #[test]
    fn rejects_source_without_main() {
        let analysis = analyze_program("fn helper() {\n}\n");
        match analysis {
            Analysis::Failed(error) => assert!(error.detail.contains("main")),
            Analysis::Analyzed(_) => panic!("expected a failure"),
        }
    }

    #[test]
    fn rejects_duplicate_main() {
        let analysis = analyze_program("fn main() {\n}\nfn main() {\n}\n");
        match analysis {
            Analysis::Failed(error) => assert!(error.detail.contains("duplicate")),
            Analysis::Analyzed(_) => panic!("expected a failure"),
        }
    }

    #[test]
    fn rejects_unclosed_function() {
        let analysis = analyze_program("fn main() {\n let a: i32 = 1;\n");
        match analysis {
            Analysis::Failed(error) => assert!(error.detail.contains("never closed")),
            Analysis::Analyzed(_) => panic!("expected a failure"),
        }
    }

    #[test]
    fn rejects_unknown_type() {
        let analysis = analyze_program("fn main() {\n let a: Thing = 1;\n}\n");
        match analysis {
            Analysis::Failed(error) => {
                assert!(error.detail.contains("unknown type"));
                assert_eq!(error.line, 2);
            }
            Analysis::Analyzed(_) => panic!("expected a failure"),
        }
    }
}
