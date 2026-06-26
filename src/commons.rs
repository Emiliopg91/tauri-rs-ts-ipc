use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use itertools::Itertools;
use quote::ToTokens;
use syn::{ExprClosure, ExprMethodCall, FnArg, Local, Pat, PatType, Type, visit::Visit};

use crate::{commands::CommandDefinition, events::EventDefinition, structs::StructDefinition};

pub fn standard_type_assoc(name: &str) -> Option<&'static str> {
    Some(match name {
        "bool" => "boolean",
        "i8" | "i16" | "i32" | "isize" => "number",
        "i64" => "string",
        "u8" | "u16" | "u32" | "u64" | "usize" => "number",
        "f32" | "f64" => "number",
        "str" | "String" => "string",
        "None" => "void",
        "()" => "void",
        _ => return None,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GenericWrapper {
    Vec,
    Map,
    Option,
    Result,
    Tuple,
}

#[derive(Debug, Clone)]
pub enum TypeRepr {
    Simple(String, String),
    Generic {
        wrapper: GenericWrapper,
        types: Vec<Option<TypeRepr>>,
    },
}

impl TypeRepr {
    pub fn from_syn_type(crate_name: &str, ty: &syn::Type) -> Option<TypeRepr> {
        match ty {
            syn::Type::Reference(r) => TypeRepr::from_syn_type(crate_name, &r.elem),
            syn::Type::Paren(p) => TypeRepr::from_syn_type(crate_name, &p.elem),
            syn::Type::Group(g) => TypeRepr::from_syn_type(crate_name, &g.elem),
            syn::Type::Tuple(t) => {
                if t.elems.len() == 0 {
                    Some(TypeRepr::Simple(crate_name.to_string(), "()".to_string()))
                } else {
                    let mut types = Vec::new();
                    for elem in &t.elems {
                        types.push(TypeRepr::from_syn_type(crate_name, &elem));
                    }

                    Some(TypeRepr::Generic {
                        wrapper: GenericWrapper::Tuple,
                        types,
                    })
                }
            }
            syn::Type::Path(type_path) => {
                let segment = type_path
                    .path
                    .segments
                    .last()
                    .expect("type path with no segments");
                let ident = segment.ident.to_string();

                match &segment.arguments {
                    syn::PathArguments::AngleBracketed(angle) => {
                        let inner_types: Vec<Option<TypeRepr>> = angle
                            .args
                            .iter()
                            .filter_map(|arg| match arg {
                                syn::GenericArgument::Type(t) => {
                                    Some(TypeRepr::from_syn_type(crate_name, t))
                                }
                                _ => None,
                            })
                            .collect();

                        let wrapper = if ident == "Vec" {
                            Some(GenericWrapper::Vec)
                        } else if ident.ends_with("Map") {
                            Some(GenericWrapper::Map)
                        } else if ident == "Option" {
                            Some(GenericWrapper::Option)
                        } else if ident == "Result" {
                            Some(GenericWrapper::Result)
                        } else {
                            None
                        };

                        match wrapper {
                            Some(w) if !inner_types.is_empty() => Some(TypeRepr::Generic {
                                wrapper: w,
                                types: inner_types,
                            }),
                            _ => {
                                eprintln!("    generic type `{ident}` not supported");
                                None
                            }
                        }
                    }
                    _ => Some(TypeRepr::Simple(crate_name.to_string(), ident)),
                }
            }
            other => {
                eprintln!("    type `{:?}` not supported", other);
                None
            }
        }
    }

    pub fn to_typescript(&self) -> String {
        match self {
            TypeRepr::Simple(_, s) => standard_type_assoc(s)
                .map(|ts| ts.to_string())
                .unwrap_or_else(|| s.clone()),
            TypeRepr::Generic { wrapper, types } => {
                let types = types
                    .iter()
                    .filter_map(|x| x.clone())
                    .collect::<Vec<TypeRepr>>();
                let first = types[0].to_typescript();
                match wrapper {
                    GenericWrapper::Vec => format!("{first}[]"),
                    GenericWrapper::Option => format!("{first} | null"),
                    GenericWrapper::Map => {
                        let second = types[1].to_typescript();
                        format!("Record<{first}, {second}>")
                    }
                    GenericWrapper::Tuple => {
                        let types = types
                            .iter()
                            .map(|t| t.to_typescript())
                            .collect::<Vec<String>>();
                        format!("[{}]", types.join(", "))
                    }
                    GenericWrapper::Result => first,
                }
            }
        }
    }

    pub fn inner_leaf_types(&self) -> Vec<String> {
        match self {
            TypeRepr::Simple(_, s) => vec![s.clone()],
            TypeRepr::Generic { types, .. } => {
                let types = types
                    .iter()
                    .filter_map(|x| x.clone())
                    .collect::<Vec<TypeRepr>>();
                types.iter().flat_map(|t| t.inner_leaf_types()).collect()
            }
        }
    }
}

pub struct RsTsVisitor {
    app_handle_vars: HashSet<String>,
    vars: HashMap<String, TypeRepr>,

    crate_name: String,

    file: PathBuf,
    syn_file: syn::File,
    base_dir: PathBuf,

    imports: HashSet<String>,
    crate_hier: String,

    pub events: Vec<EventDefinition>,
    pub commands: Vec<CommandDefinition>,
    pub structs: Vec<StructDefinition>,
}

impl RsTsVisitor {
    pub fn new<F, B>(file: &(F, syn::File), base_dir: B) -> Self
    where
        B: AsRef<Path>,
        F: AsRef<Path>,
    {
        let mut crate_name = PathBuf::from({
            file.0
                .as_ref()
                .display()
                .to_string()
                .replace(&base_dir.as_ref().display().to_string(), "")
        })
        .display()
        .to_string()
        .replace(&base_dir.as_ref().display().to_string(), "")
        .replace("/", "::");
        crate_name = crate_name[0..crate_name.len() - 3].to_string();
        if crate_name.ends_with("mod") {
            crate_name = crate_name[0..crate_name.len() - 5].to_string();
        }
        if crate_name.starts_with("::") {
            crate_name = crate_name[2..crate_name.len()].to_string();
        }
        crate_name = format!("crate::{}", crate_name);

        Self {
            crate_name,
            app_handle_vars: HashSet::new(),
            vars: HashMap::new(),
            events: Vec::new(),
            base_dir: base_dir.as_ref().to_path_buf(),
            file: file.0.as_ref().to_path_buf(),
            syn_file: file.1.clone(),
            commands: Vec::new(),
            structs: Vec::new(),
            imports: HashSet::new(),
            crate_hier: String::new(),
        }
    }

    fn extract_pat_ident(pat: &Pat) -> Option<String> {
        match pat {
            Pat::Ident(ident) => Some(ident.ident.to_string()),
            _ => None,
        }
    }

    fn is_app_type(ty: &syn::Type) -> bool {
        ty.to_token_stream().to_string().contains("AppHandle")
            || ty.to_token_stream().to_string().contains("App")
    }

    fn type_as_path_string(ty: &Type) -> Option<String> {
        match ty {
            Type::Path(type_path) => {
                let s = type_path
                    .path
                    .segments
                    .iter()
                    .map(|seg| seg.ident.to_string())
                    .collect::<Vec<_>>()
                    .join("::");
                Some(s)
            }
            _ => None,
        }
    }

    fn get_type_repr(&self, ty_orig: &Type) -> Option<TypeRepr> {
        if let Some(ty_str) = Self::type_as_path_string(ty_orig) {
            if let Some(std) = standard_type_assoc(ty_str.split("::").last().unwrap()) {
                Some(TypeRepr::Simple("".to_string(), std.to_string()))
            } else {
                TypeRepr::from_syn_type("", ty_orig)
            }
        } else {
            None
        }
    }
}

impl<'ast> Visit<'ast> for RsTsVisitor {
    fn visit_item_struct(&mut self, struct_def: &'ast syn::ItemStruct) {
        self.structs.push(StructDefinition::from_item_struct(
            struct_def,
            &self.base_dir,
            &self.file,
            &self.syn_file,
            &self.crate_name,
            &self.imports,
        ));
    }

    fn visit_item_fn(&mut self, fn_def: &'ast syn::ItemFn) {
        if let Some(cmd) = CommandDefinition::from_item_fn(
            fn_def,
            &self.imports,
            &self.base_dir,
            &self.file,
            &self.syn_file,
        ) {
            self.commands.push(cmd);
        }
        syn::visit::visit_item_fn(self, fn_def);
    }

    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if let Some(event) = EventDefinition::from_expr_method_call(
            node,
            &self.app_handle_vars,
            &self.vars,
            &self.imports,
            &self.file,
            &self.syn_file,
        ) {
            self.events.push(event);
        }
        syn::visit::visit_expr_method_call(self, node);
    }

    fn visit_use_path(&mut self, path: &'ast syn::UsePath) {
        let prev = self.crate_hier.clone();
        self.crate_hier = format!("{}::{}", self.crate_hier, path.ident);
        syn::visit::visit_use_path(self, path);
        self.crate_hier = prev;
    }

    fn visit_use_name(&mut self, name: &'ast syn::UseName) {
        let full = format!("{}::{}", self.crate_hier, name.ident);
        syn::visit::visit_use_name(self, name);
        self.imports
            .insert(full.strip_prefix("::").unwrap().to_string());
    }

    fn visit_fn_arg(&mut self, node: &'ast FnArg) {
        if let FnArg::Typed(pat_type) = node
            && let Some(name) = Self::extract_pat_ident(&pat_type.pat)
        {
            if Self::is_app_type(&pat_type.ty) {
                self.app_handle_vars.insert(name);
            } else if Self::type_as_path_string(&pat_type.ty).is_some()
                && let Some(ty) = self.get_type_repr(&pat_type.ty)
            {
                self.vars.insert(name, ty);
            }
        }
        syn::visit::visit_fn_arg(self, node);
    }

    fn visit_local(&mut self, node: &'ast Local) {
        match &node.pat {
            Pat::Type(PatType { ty, pat, .. }) => {
                if let Some(name) = Self::extract_pat_ident(pat) {
                    if Self::is_app_type(ty) {
                        self.app_handle_vars.insert(name);
                    } else if Self::type_as_path_string(ty).is_some()
                        && let Some(ty) = self.get_type_repr(ty)
                    {
                        self.vars.insert(name, ty);
                    }
                }
            }
            Pat::Ident(pat_ident) => {
                let name = pat_ident.ident.to_string();
                if let Some(init) = &node.init {
                    let init_repr = init.expr.to_token_stream().to_string();
                    if self.app_handle_vars.contains(&init_repr) {
                        self.app_handle_vars.insert(name);
                    }
                }
            }
            _ => {}
        }
        syn::visit::visit_local(self, node);
    }

    fn visit_expr_closure(&mut self, node: &'ast ExprClosure) {
        for input in &node.inputs {
            if let Pat::Type(PatType { ty, pat, .. }) = input
                && Self::is_app_type(ty)
                && let Some(name) = Self::extract_pat_ident(pat)
            {
                self.app_handle_vars.insert(name);
            }
        }
        syn::visit::visit_expr_closure(self, node);
    }
}
