use crate::model::ids::{FrameId, ScopeId};

/// Why a scope exists. Function bodies and plain `{}` blocks drop the same way;
/// the kind is kept so the drawing can label the box.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ScopeKind {
    Function,
    Block,
    Loop,
}

/// One open lexical scope.
#[derive(Clone, PartialEq, Debug)]
pub struct Scope {
    pub id: ScopeId,
    pub kind: ScopeKind,
    /// Frame that owns the scope. A scope never outlives its frame.
    pub frame: FrameId,
    pub parent: Option<ScopeId>,
    /// 0 for a function body, 1 for a block inside it, and so on.
    pub depth: u32,
    /// Line of the opening brace.
    pub line: u32,
    /// Names in declaration order. `Drop` runs over this vector backwards.
    pub variables: Vec<String>,
}

/// A scope that was just closed, with the variables it dropped.
#[derive(Clone, PartialEq, Debug)]
pub struct ClosedScope {
    pub scope: Scope,
    /// Reverse declaration order: the first name is the first one dropped.
    pub dropped: Vec<String>,
}

/// Where a name lives, from the point of view of the running block.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Resolved {
    pub scope: ScopeId,
    pub depth: u32,
}

#[derive(Clone, PartialEq, Debug)]
pub struct ScopeRegistry {
    /// Every scope ever opened, in opening order. Closed scopes stay for replay.
    scopes: Vec<Scope>,
    /// Open scopes, outermost first. The last one is the running scope.
    stack: Vec<ScopeId>,
    next_id: u32,
}

impl ScopeRegistry {
    pub fn new() -> Self {
        ScopeRegistry {
            scopes: Vec::new(),
            stack: Vec::new(),
            next_id: 1,
        }
    }

    /// Opens a scope inside the current one and makes it the running scope.
    pub fn open(&mut self, kind: ScopeKind, frame: FrameId, line: u32) -> ScopeId {
        let id = ScopeId(self.next_id);
        self.next_id += 1;
        self.push_scope(id, kind, frame, line);
        id
    }

    /// Opens a scope with the id the recorder announced. Returns `None` when that
    /// id was already used, so a replay cannot alias two different scopes.
    pub fn open_as(
        &mut self,
        id: ScopeId,
        kind: ScopeKind,
        frame: FrameId,
        line: u32,
    ) -> Option<ScopeId> {
        if self.scopes.iter().any(|scope| scope.id == id) {
            return None;
        }
        self.next_id = self.next_id.max(id.0.saturating_add(1));
        self.push_scope(id, kind, frame, line);
        Some(id)
    }

    fn push_scope(&mut self, id: ScopeId, kind: ScopeKind, frame: FrameId, line: u32) {
        let parent = self.stack.last().copied();
        let depth = parent
            .and_then(|parent| self.scope(parent))
            .map_or(0, |scope| scope.depth + 1);
        self.scopes.push(Scope {
            id,
            kind,
            frame,
            parent,
            depth,
            line,
            variables: Vec::new(),
        });
        self.stack.push(id);
    }

    /// Declares a name in the running scope. Returns `None` when no scope is open.
    /// A repeated name is ignored: the engine keeps the first declaration, which
    /// is the one whose lifetime the trace already announced.
    pub fn declare(&mut self, name: impl Into<String>) -> Option<ScopeId> {
        let id = self.stack.last().copied()?;
        let name = name.into();
        let scope = self.scope_mut(id)?;
        if !scope.variables.contains(&name) {
            scope.variables.push(name);
        }
        Some(id)
    }

    /// Closes the running scope. The variables come back in drop order.
    pub fn close(&mut self) -> Option<ClosedScope> {
        let id = self.stack.pop()?;
        let scope = self.scopes.iter().find(|scope| scope.id == id)?.clone();
        let dropped: Vec<String> = scope.variables.iter().rev().cloned().collect();
        Some(ClosedScope { scope, dropped })
    }

    /// Closes every scope up to and including `target`, outermost first. A
    /// function return uses this: each closed scope drops its own variables.
    pub fn close_until(&mut self, target: ScopeId) -> Vec<ClosedScope> {
        if !self.stack.contains(&target) {
            return Vec::new();
        }
        let mut closed = Vec::new();
        while let Some(scope) = self.close() {
            let reached = scope.scope.id == target;
            closed.push(scope);
            if reached {
                break;
            }
        }
        closed
    }

    /// Closes every open scope that belongs to `frame`, innermost first. A
    /// frame can never outlive its scopes, so `EndFunction` calls this.
    pub fn close_frame(&mut self, frame: FrameId) -> Vec<ClosedScope> {
        let mut closed = Vec::new();
        while let Some(current) = self.current() {
            if current.frame != frame {
                break;
            }
            match self.close() {
                Some(scope) => closed.push(scope),
                None => break,
            }
        }
        closed
    }

    pub fn scope(&self, id: ScopeId) -> Option<&Scope> {
        self.scopes.iter().find(|scope| scope.id == id)
    }

    pub fn scope_mut(&mut self, id: ScopeId) -> Option<&mut Scope> {
        self.scopes.iter_mut().find(|scope| scope.id == id)
    }

    pub fn current(&self) -> Option<&Scope> {
        self.stack.last().and_then(|id| self.scope(*id))
    }

    pub fn is_open(&self, id: ScopeId) -> bool {
        self.stack.contains(&id)
    }

    pub fn open_scopes(&self) -> &[ScopeId] {
        &self.stack
    }

    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// Innermost open scope that declares `name`, plus its depth.
    pub fn resolve(&self, name: &str) -> Option<Resolved> {
        self.stack
            .iter()
            .rev()
            .filter_map(|id| self.scope(*id))
            .find(|scope| scope.variables.iter().any(|variable| variable == name))
            .map(|scope| Resolved {
                scope: scope.id,
                depth: scope.depth,
            })
    }

    /// Number of scopes ever opened, closed ones included.
    pub fn len(&self) -> usize {
        self.scopes.len()
    }
}

impl Default for ScopeRegistry {
    fn default() -> Self {
        ScopeRegistry::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> FrameId {
        FrameId(1)
    }

    #[test]
    fn a_new_scope_becomes_the_running_one() {
        let mut scopes = ScopeRegistry::new();
        let function = scopes.open(ScopeKind::Function, frame(), 1);
        assert_eq!(scopes.current().map(|scope| scope.id), Some(function));
        assert_eq!(scopes.current().map(|scope| scope.depth), Some(0));
        assert_eq!(scopes.depth(), 1);
    }

    #[test]
    fn a_block_nests_inside_the_function() {
        let mut scopes = ScopeRegistry::new();
        let function = scopes.open(ScopeKind::Function, frame(), 1);
        let block = scopes.open(ScopeKind::Block, frame(), 3);
        let innermost = scopes.current().expect("block is open");
        assert_eq!(innermost.parent, Some(function));
        assert_eq!(innermost.kind, ScopeKind::Block);
        assert_eq!(innermost.depth, 1);
        assert_eq!(scopes.open_scopes(), &[function, block]);
        assert!(scopes.is_open(function));
        assert!(scopes.is_open(block));
    }

    #[test]
    fn closing_returns_variables_in_reverse_declaration_order() {
        let mut scopes = ScopeRegistry::new();
        scopes.open(ScopeKind::Function, frame(), 1);
        scopes.declare("a");
        let block = scopes.open(ScopeKind::Block, frame(), 2);
        scopes.declare("x");
        scopes.declare("y");
        scopes.declare("z");

        let closed = scopes.close().expect("the block closes first");
        assert_eq!(closed.scope.id, block);
        assert_eq!(closed.dropped, vec!["z", "y", "x"]);
        assert_eq!(scopes.current().map(|scope| scope.id), Some(ScopeId(1)));
    }

    #[test]
    fn closing_the_function_drops_the_outer_variables_last() {
        let mut scopes = ScopeRegistry::new();
        scopes.open(ScopeKind::Function, frame(), 1);
        scopes.declare("first");
        scopes.declare("second");

        let closed = scopes.close().expect("the function closes");
        assert_eq!(closed.dropped, vec!["second", "first"]);
        assert!(scopes.current().is_none());
        assert_eq!(scopes.depth(), 0);
    }

    #[test]
    fn a_return_closes_every_scope_up_to_the_function() {
        let mut scopes = ScopeRegistry::new();
        let function = scopes.open(ScopeKind::Function, frame(), 1);
        scopes.declare("outer");
        scopes.open(ScopeKind::Block, frame(), 3);
        scopes.declare("inner");
        scopes.open(ScopeKind::Loop, frame(), 4);
        scopes.declare("item");

        let closed = scopes.close_until(function);
        assert_eq!(closed.len(), 3);
        assert_eq!(closed[0].dropped, vec!["item"]);
        assert_eq!(closed[1].dropped, vec!["inner"]);
        assert_eq!(closed[2].dropped, vec!["outer"]);
        assert!(scopes.is_empty());
    }

    #[test]
    fn resolving_a_name_prefers_the_innermost_scope() {
        let mut scopes = ScopeRegistry::new();
        scopes.open(ScopeKind::Function, frame(), 1);
        scopes.declare("value");
        let block = scopes.open(ScopeKind::Block, frame(), 3);
        scopes.declare("value");
        scopes.declare("only_here");

        let found = scopes.resolve("value").expect("value is declared twice");
        assert_eq!(found.scope, block);
        assert_eq!(found.depth, 1);
        assert_eq!(scopes.resolve("missing"), None);
    }

    #[test]
    fn closing_an_empty_registry_is_harmless() {
        let mut scopes = ScopeRegistry::new();
        assert_eq!(scopes.close(), None);
        assert_eq!(scopes.declare("orphan"), None);
        assert_eq!(scopes.close_until(ScopeId(9)), Vec::new());
    }
}
