//! C++ template instantiation via monomorphization.
//!
//! C++ templates are compiled using monomorphization: for each unique set of
//! type arguments, a new specialization is generated. For example,
//! `vector<int>` and `vector<double>` produce two distinct concrete types
//! with independent machine code.
//!
//! ## Monomorphization Process
//!
//! 1. **Registration**: Template definitions (function or class) are stored
//!    alongside their template parameter lists (`template<typename T>`).
//!
//! 2. **Instantiation request**: When code uses `foo<int>(42)`, the engine
//!    looks up the template `foo` and the concrete type argument `int`.
//!
//! 3. **Substitution**: A substitution map is built (`T -> int`), and the
//!    template AST is cloned. In a full implementation, every occurrence of
//!    `T` in the AST would be replaced with `int`.
//!
//! 4. **Name mangling**: The instantiated function gets a unique mangled name
//!    like `identity_Int` to avoid symbol collisions.
//!
//! 5. **Deduplication**: The `instantiated` map tracks which (template, args)
//!    pairs have already been instantiated, preventing duplicate codegen.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use alloc::format;

use crate::ast::*;

/// A key identifying a specific template instantiation.
///
/// Two instantiation requests with the same template name and type arguments
/// (stringified for comparison) are considered identical and will share code.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct InstantiationKey {
    /// Name of the template being instantiated.
    pub name: String,
    /// Stringified concrete type arguments (e.g., `["Int", "Double"]`).
    pub args: Vec<String>,
}

/// Template instantiation engine.
///
/// Stores registered function and class templates, tracks which have been
/// instantiated, and generates concrete specializations on demand.
pub struct TemplateEngine {
    /// Function templates: name -> template def.
    func_templates: BTreeMap<String, CppFuncDef>,
    /// Class templates: name -> template def.
    class_templates: BTreeMap<String, ClassDef>,
    /// Template parameter lists.
    template_params: BTreeMap<String, Vec<TemplateParam>>,
    /// Already instantiated templates (avoid duplicates).
    instantiated: BTreeMap<InstantiationKey, String>,
}

impl TemplateEngine {
    pub fn new() -> Self {
        Self {
            func_templates: BTreeMap::new(),
            class_templates: BTreeMap::new(),
            template_params: BTreeMap::new(),
            instantiated: BTreeMap::new(),
        }
    }

    /// Register a function template.
    pub fn register_func_template(&mut self, name: String, params: Vec<TemplateParam>, func: CppFuncDef) {
        self.template_params.insert(name.clone(), params);
        self.func_templates.insert(name, func);
    }

    /// Register a class template.
    pub fn register_class_template(&mut self, name: String, params: Vec<TemplateParam>, class: ClassDef) {
        self.template_params.insert(name.clone(), params);
        self.class_templates.insert(name, class);
    }

    /// Instantiate a function template with concrete types.
    pub fn instantiate_func(&mut self, name: &str, type_args: &[CppType]) -> Result<CppFuncDef, String> {
        let key = InstantiationKey {
            name: String::from(name),
            args: type_args.iter().map(|t| format!("{:?}", t)).collect(),
        };

        if let Some(mangled) = self.instantiated.get(&key) {
            // Already instantiated
            let func = self.func_templates.get(name)
                .ok_or_else(|| format!("template '{}' not found", name))?
                .clone();
            let mut result = func;
            result.name = mangled.clone();
            return Ok(result);
        }

        let template = self.func_templates.get(name)
            .ok_or_else(|| format!("function template '{}' not found", name))?
            .clone();
        let params = self.template_params.get(name)
            .ok_or_else(|| format!("template params for '{}' not found", name))?
            .clone();

        // Build substitution map: T -> concrete type
        let mut subst: BTreeMap<String, CppType> = BTreeMap::new();
        for (i, param) in params.iter().enumerate() {
            if let Some(arg) = type_args.get(i) {
                subst.insert(param.name.clone(), arg.clone());
            }
        }

        // Generate mangled name
        let mangled_name = format!("{}_{}", name,
            type_args.iter().map(|t| format!("{:?}", t)).collect::<Vec<_>>().join("_"));

        self.instantiated.insert(key, mangled_name.clone());

        let mut result = template;
        result.name = mangled_name;
        // In a full implementation, we'd walk the AST and substitute types
        Ok(result)
    }

    /// Instantiate a class template with concrete types.
    pub fn instantiate_class(&mut self, name: &str, type_args: &[CppType]) -> Result<ClassDef, String> {
        let key = InstantiationKey {
            name: String::from(name),
            args: type_args.iter().map(|t| format!("{:?}", t)).collect(),
        };

        if self.instantiated.contains_key(&key) {
            let class = self.class_templates.get(name)
                .ok_or_else(|| format!("class template '{}' not found", name))?
                .clone();
            return Ok(class);
        }

        let template = self.class_templates.get(name)
            .ok_or_else(|| format!("class template '{}' not found", name))?
            .clone();

        let mangled_name = format!("{}_{}", name,
            type_args.iter().map(|t| format!("{:?}", t)).collect::<Vec<_>>().join("_"));

        self.instantiated.insert(key, mangled_name.clone());

        let mut result = template;
        result.name = mangled_name;
        Ok(result)
    }

    /// Check if a template exists.
    pub fn has_template(&self, name: &str) -> bool {
        self.func_templates.contains_key(name) || self.class_templates.contains_key(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_register_and_instantiate() {
        let mut engine = TemplateEngine::new();

        let func = CppFuncDef {
            name: String::from("identity"),
            qualified_name: None,
            return_type: CppType::Named(String::from("T")),
            params: Vec::new(),
            body: CppBlock { stmts: Vec::new() },
            is_constexpr: false,
            is_noexcept: false,
            template_params: Vec::new(),
            span: crate::lexer::Span::default(),
        };

        engine.register_func_template(
            String::from("identity"),
            alloc::vec![TemplateParam {
                name: String::from("T"),
                is_typename: true,
                default: None,
            }],
            func,
        );

        let result = engine.instantiate_func("identity", &[CppType::Int]);
        assert!(result.is_ok());
        assert!(result.unwrap().name.contains("identity"));
    }
}
