//! TypeScript type system and type checker.
//!
//! Supports: primitive types (string, number, boolean, void, null, undefined,
//! never, any, unknown), union types (A | B), intersection types (A & B),
//! generic types, interface types, enum types, array types, tuple types,
//! literal types, type narrowing (typeof, instanceof, in), type assertions.
//!
//! Type checking is advisory (warnings, not errors) for gradual typing.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// TypeScript type representation.
#[derive(Debug, Clone, PartialEq)]
pub enum TsType {
    // === Primitive types ===
    String,
    Number,
    Boolean,
    Void,
    Null,
    Undefined,
    Never,
    Any,
    Unknown,
    BigInt,
    Symbol,

    // === Literal types ===
    StringLiteral(String),
    NumberLiteral(f64),
    BooleanLiteral(bool),

    // === Composite types ===
    /// Array: T[]
    Array(Box<TsType>),
    /// Tuple: [T, U, V]
    Tuple(Vec<TsType>),
    /// Union: A | B
    Union(Vec<TsType>),
    /// Intersection: A & B
    Intersection(Vec<TsType>),
    /// Object type / interface
    Object(Vec<ObjectMember>),
    /// Function type: (params) => return
    Function {
        params: Vec<FuncParam>,
        returns: Box<TsType>,
        type_params: Vec<TypeParam>,
    },
    /// Generic instantiation: T<U>
    Generic {
        base: Box<TsType>,
        args: Vec<TsType>,
    },
    /// Type parameter: T
    TypeParam(String),
    /// Named type reference
    Named(String),
    /// Qualified name: Ns.Type
    Qualified(String, String),
    /// keyof T
    Keyof(Box<TsType>),
    /// typeof x
    Typeof(String),
    /// Mapped type: { [K in T]: U }
    Mapped {
        param: String,
        constraint: Box<TsType>,
        value: Box<TsType>,
    },
    /// Conditional type: T extends U ? X : Y
    Conditional {
        check: Box<TsType>,
        extends: Box<TsType>,
        true_type: Box<TsType>,
        false_type: Box<TsType>,
    },
    /// Index access: T[K]
    IndexAccess {
        object: Box<TsType>,
        index: Box<TsType>,
    },
    /// Template literal type: `${T}`
    TemplateLiteral(Vec<TsType>),
}

/// Object type member.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectMember {
    pub name: String,
    pub ty: TsType,
    pub optional: bool,
    pub readonly: bool,
}

/// Function parameter with type.
#[derive(Debug, Clone, PartialEq)]
pub struct FuncParam {
    pub name: String,
    pub ty: TsType,
    pub optional: bool,
    pub rest: bool,
}

/// Type parameter with optional constraint and default.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeParam {
    pub name: String,
    pub constraint: Option<TsType>,
    pub default: Option<TsType>,
}

/// Type checking diagnostic.
#[derive(Debug, Clone)]
pub struct TypeDiagnostic {
    pub message: String,
    pub severity: Severity,
    pub line: u32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Type checker context.
pub struct TypeChecker {
    /// Type environment: variable name -> type.
    pub env: BTreeMap<String, TsType>,
    /// Interface definitions.
    pub interfaces: BTreeMap<String, Vec<ObjectMember>>,
    /// Type aliases.
    pub type_aliases: BTreeMap<String, TsType>,
    /// Enum definitions: name -> variants.
    pub enums: BTreeMap<String, Vec<(String, Option<i64>)>>,
    /// Diagnostics collected during checking.
    pub diagnostics: Vec<TypeDiagnostic>,
    /// Generic type parameters in scope.
    pub type_params: Vec<BTreeMap<String, Option<TsType>>>,
}

impl TypeChecker {
    pub fn new() -> Self {
        Self {
            env: BTreeMap::new(),
            interfaces: BTreeMap::new(),
            type_aliases: BTreeMap::new(),
            enums: BTreeMap::new(),
            diagnostics: Vec::new(),
            type_params: Vec::new(),
        }
    }

    /// Add a warning diagnostic.
    pub fn warn(&mut self, msg: String, line: u32) {
        self.diagnostics.push(TypeDiagnostic {
            message: msg,
            severity: Severity::Warning,
            line,
        });
    }

    /// Check if type A is assignable to type B.
    pub fn is_assignable(&self, from: &TsType, to: &TsType) -> bool {
        // any is assignable to/from anything
        if matches!(from, TsType::Any) || matches!(to, TsType::Any) {
            return true;
        }
        // unknown accepts everything
        if matches!(to, TsType::Unknown) {
            return true;
        }
        // never is assignable to anything
        if matches!(from, TsType::Never) {
            return true;
        }
        // Same type
        if from == to {
            return true;
        }
        // null/undefined are assignable to void
        if matches!(from, TsType::Null | TsType::Undefined) && matches!(to, TsType::Void) {
            return true;
        }
        // Union: from is assignable to union if assignable to any member
        if let TsType::Union(members) = to {
            return members.iter().any(|m| self.is_assignable(from, m));
        }
        // From union: all members must be assignable
        if let TsType::Union(members) = from {
            return members.iter().all(|m| self.is_assignable(m, to));
        }
        // Literal types are assignable to their base
        match (from, to) {
            (TsType::StringLiteral(_), TsType::String) => true,
            (TsType::NumberLiteral(_), TsType::Number) => true,
            (TsType::BooleanLiteral(_), TsType::Boolean) => true,
            // Array covariance (simplified)
            (TsType::Array(a), TsType::Array(b)) => self.is_assignable(a, b),
            // Object structural typing
            (TsType::Object(from_members), TsType::Object(to_members)) => {
                to_members.iter().all(|to_m| {
                    from_members.iter().any(|from_m| {
                        from_m.name == to_m.name && self.is_assignable(&from_m.ty, &to_m.ty)
                    }) || to_m.optional
                })
            }
            _ => false,
        }
    }

    /// Narrow a type based on a typeof check.
    pub fn narrow_typeof(&self, ty: &TsType, typeof_str: &str) -> TsType {
        match typeof_str {
            "string" => TsType::String,
            "number" => TsType::Number,
            "boolean" => TsType::Boolean,
            "undefined" => TsType::Undefined,
            "function" => TsType::Function {
                params: Vec::new(),
                returns: Box::new(TsType::Any),
                type_params: Vec::new(),
            },
            "object" => TsType::Object(Vec::new()),
            "bigint" => TsType::BigInt,
            "symbol" => TsType::Symbol,
            _ => ty.clone(),
        }
    }

    /// Resolve a type alias.
    pub fn resolve_type(&self, ty: &TsType) -> TsType {
        match ty {
            TsType::Named(name) => {
                if let Some(resolved) = self.type_aliases.get(name) {
                    self.resolve_type(resolved)
                } else {
                    ty.clone()
                }
            }
            _ => ty.clone(),
        }
    }

    /// Register an interface definition.
    pub fn define_interface(&mut self, name: String, members: Vec<ObjectMember>) {
        // Merge with existing (declaration merging)
        let existing = self.interfaces.entry(name).or_insert_with(Vec::new);
        existing.extend(members);
    }

    /// Register a type alias.
    pub fn define_type_alias(&mut self, name: String, ty: TsType) {
        self.type_aliases.insert(name, ty);
    }

    /// Register an enum.
    pub fn define_enum(&mut self, name: String, variants: Vec<(String, Option<i64>)>) {
        self.enums.insert(name, variants);
    }

    /// Get the type of a variable from the environment.
    pub fn lookup_var(&self, name: &str) -> TsType {
        self.env.get(name).cloned().unwrap_or(TsType::Any)
    }

    /// Set a variable's type.
    pub fn set_var(&mut self, name: String, ty: TsType) {
        self.env.insert(name, ty);
    }

    /// Get all warnings as a formatted string.
    pub fn format_diagnostics(&self) -> String {
        let mut out = String::new();
        for d in &self.diagnostics {
            let prefix = match d.severity {
                Severity::Error => "error",
                Severity::Warning => "warning",
                Severity::Info => "info",
            };
            out.push_str(&alloc::format!("{}(line {}): {}\n", prefix, d.line, d.message));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_any_assignable() {
        let tc = TypeChecker::new();
        assert!(tc.is_assignable(&TsType::Any, &TsType::String));
        assert!(tc.is_assignable(&TsType::Number, &TsType::Any));
    }

    #[test]
    fn test_union_assignable() {
        let tc = TypeChecker::new();
        let union = TsType::Union(alloc::vec![TsType::String, TsType::Number]);
        assert!(tc.is_assignable(&TsType::String, &union));
        assert!(tc.is_assignable(&TsType::Number, &union));
        assert!(!tc.is_assignable(&TsType::Boolean, &union));
    }

    #[test]
    fn test_literal_assignable() {
        let tc = TypeChecker::new();
        assert!(tc.is_assignable(
            &TsType::StringLiteral(String::from("hello")),
            &TsType::String
        ));
    }

    #[test]
    fn test_never_assignable() {
        let tc = TypeChecker::new();
        assert!(tc.is_assignable(&TsType::Never, &TsType::String));
        assert!(tc.is_assignable(&TsType::Never, &TsType::Number));
    }

    #[test]
    fn test_structural_typing() {
        let tc = TypeChecker::new();
        let from = TsType::Object(alloc::vec![
            ObjectMember { name: String::from("x"), ty: TsType::Number, optional: false, readonly: false },
            ObjectMember { name: String::from("y"), ty: TsType::Number, optional: false, readonly: false },
        ]);
        let to = TsType::Object(alloc::vec![
            ObjectMember { name: String::from("x"), ty: TsType::Number, optional: false, readonly: false },
        ]);
        assert!(tc.is_assignable(&from, &to));
    }

    #[test]
    fn test_narrow_typeof() {
        let tc = TypeChecker::new();
        let any = TsType::Any;
        assert_eq!(tc.narrow_typeof(&any, "string"), TsType::String);
        assert_eq!(tc.narrow_typeof(&any, "number"), TsType::Number);
    }
}
