use crate::model::ids::{FunctionId, StructId};

/// Kind of a runtime value, used to compare it against a declared type without
/// copying the declared type into the memory model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ValueKind {
    Integer,
    Float,
    Boolean,
    Character,
    Text,
    Structure(StructId),
    Sequence,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DataType {
    Integer,
    Float,
    Boolean,
    Character,
    Text,
    Structure(StructId),
    Sequence(Box<DataType>),
}

impl DataType {
    pub fn kind(&self) -> ValueKind {
        match self {
            DataType::Integer => ValueKind::Integer,
            DataType::Float => ValueKind::Float,
            DataType::Boolean => ValueKind::Boolean,
            DataType::Character => ValueKind::Character,
            DataType::Text => ValueKind::Text,
            DataType::Structure(structure) => ValueKind::Structure(*structure),
            DataType::Sequence(_) => ValueKind::Sequence,
        }
    }
}

/// Field of a struct definition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FieldDef {
    pub name: String,
    pub data_type: DataType,
}

/// Parameter or local variable of a function.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Declaration {
    pub name: String,
    pub data_type: DataType,
    pub line: u32,
}

/// Edge of the call graph drawn by the UI.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CallSite {
    pub target: FunctionId,
    pub line: u32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FunctionDef {
    pub id: FunctionId,
    pub name: String,
    pub start_line: u32,
    pub parameters: Vec<Declaration>,
    pub declarations: Vec<Declaration>,
    pub calls: Vec<CallSite>,
}

impl FunctionDef {
    /// Looks in parameters first, then in local declarations.
    pub fn declaration_named(&self, name: &str) -> Option<&Declaration> {
        self.parameters
            .iter()
            .chain(self.declarations.iter())
            .find(|declaration| declaration.name == name)
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StructDef {
    pub id: StructId,
    pub name: String,
    pub fields: Vec<FieldDef>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ProgramTree {
    /// Function whose frame is created first. Exactly one `main` exists.
    pub entry: FunctionId,
    pub functions: Vec<FunctionDef>,
    pub structs: Vec<StructDef>,
}

impl ProgramTree {
    pub fn function(&self, id: FunctionId) -> Option<&FunctionDef> {
        self.functions.iter().find(|function| function.id == id)
    }

    pub fn structure(&self, id: StructId) -> Option<&StructDef> {
        self.structs.iter().find(|structure| structure.id == id)
    }

    pub fn function_named(&self, name: &str) -> Option<FunctionId> {
        self.functions
            .iter()
            .find(|function| function.name == name)
            .map(|function| function.id)
    }

    pub fn find_declaration(&self, function: FunctionId, name: &str) -> Option<&Declaration> {
        self.function(function)
            .and_then(|definition| definition.declaration_named(name))
    }

    pub fn find_field(&self, structure: StructId, name: &str) -> Option<&FieldDef> {
        self.structure(structure)
            .and_then(|definition| definition.fields.iter().find(|field| field.name == name))
    }
}
