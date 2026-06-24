use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use quote::ToTokens;
use syn::{
    ExprClosure, ExprMethodCall, FnArg, Local, Pat, PatType, Type, spanned::Spanned, visit::Visit,
};

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
            syn::Type::Tuple(t) if t.elems.is_empty() => {
                Some(TypeRepr::Simple(crate_name.to_string(), "()".to_string()))
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
                                _ => None, // ignora lifetimes, const generics, etc.
                            })
                            .collect();

                        let wrapper = if ident == "Vec" {
                            Some(GenericWrapper::Vec)
                        } else if ident.ends_with("Map") {
                            // HashMap, BTreeMap, IndexMap... igual que el original
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
                                eprintln!("generic type `{ident}` not supported");
                                None
                            }
                        }
                    }
                    _ => Some(TypeRepr::Simple(crate_name.to_string(), ident)),
                }
            }
            other => {
                eprintln!("type `{:?}` not supported", other);
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
                    .into_iter()
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
                    .into_iter()
                    .filter_map(|x| x.clone())
                    .collect::<Vec<TypeRepr>>();
                types.iter().flat_map(|t| t.inner_leaf_types()).collect()
            }
        }
    }
}

pub fn collect_imports<T>(file: T) -> Vec<String>
where
    T: AsRef<Path>,
{
    let content = fs::read_to_string(file.as_ref()).unwrap();
    let file = syn::parse_file(&content).unwrap();
    let mut imports = Vec::new();

    for item in &file.items {
        if let syn::Item::Use(use_item) = item {
            collect_use_tree(&use_item.tree, String::new(), &mut imports);
        }
    }

    imports
}

fn collect_use_tree(tree: &syn::UseTree, prefix: String, imports: &mut Vec<String>) {
    match tree {
        syn::UseTree::Path(path) => {
            let new_prefix = if prefix.is_empty() {
                path.ident.to_string()
            } else {
                format!("{}::{}", prefix, path.ident)
            };

            collect_use_tree(&path.tree, new_prefix, imports);
        }

        syn::UseTree::Name(name) => {
            let full = format!("{}::{}", prefix, name.ident);
            if !imports.contains(&full) {
                imports.push(full);
            }
        }

        syn::UseTree::Rename(rename) => {
            let full = format!("{}::{}", prefix, rename.ident);
            if !imports.contains(&full) {
                imports.push(full);
            }
        }

        syn::UseTree::Group(group) => {
            for item in &group.items {
                collect_use_tree(item, prefix.clone(), imports);
            }
        }

        syn::UseTree::Glob(_) => {
            // use foo::*;
        }
    }
}

pub struct RsTsVisitor {
    app_handle_vars: HashSet<String>,
    vars: HashMap<String, TypeRepr>,

    crate_name: String,
    file: PathBuf,
    base_dir: PathBuf,
    imports: Vec<String>,

    pub events: Vec<EventDefinition>,
    pub commands: Vec<CommandDefinition>,
    pub structs: Vec<StructDefinition>,
}

impl RsTsVisitor {
    pub fn new<F, B>(file: F, base_dir: B) -> Self
    where
        B: AsRef<Path>,
        F: AsRef<Path>,
    {
        let mut crate_name = PathBuf::from({
            let f = file
                .as_ref()
                .display()
                .to_string()
                .replace(&base_dir.as_ref().display().to_string(), "");
            f
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
            imports: collect_imports(&file),
            base_dir: base_dir.as_ref().to_path_buf(),
            file: file.as_ref().to_path_buf(),
            commands: Vec::new(),
            structs: Vec::new(),
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
                return TypeRepr::from_syn_type("", ty_orig);
            }
        } else {
            None
        }
    }

    fn has_tauri_command_attr(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|attr| {
            let path = attr.path();
            path.segments.len() == 2
                && path.segments[0].ident == "tauri"
                && path.segments[1].ident == "command"
        })
    }

    fn is_type_excluded(ty: &syn::Type) -> bool {
        quote::quote!(#ty).to_string().contains("AppHandle")
    }

    pub fn find<F, B>(&self, file: F, base_dir: B)
    where
        F: AsRef<Path>,
        B: AsRef<Path>,
    {
        println!("{}", file.as_ref().display());
        let content = fs::read_to_string(file.as_ref()).unwrap();
        let file_syn = syn::parse_file(&content).unwrap();
        let items = &file_syn.items;

        for item in items {
            if let syn::Item::Struct(struct_def) = item {
                let name = struct_def.ident.to_string();
                let mut fields = HashMap::new();
                let location = format!(
                    "Definition: {}:{}",
                    file.as_ref()
                        .display()
                        .to_string()
                        .replace(&base_dir.as_ref().display().to_string(), ""),
                    struct_def.struct_token.span().start().line
                );
                for field in &struct_def.fields {
                    if let Some(type_rep) = TypeRepr::from_syn_type("", &field.ty) {
                        fields
                            .entry(field.ident.as_ref().unwrap().to_string())
                            .or_insert(type_rep);
                    }
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for RsTsVisitor {
    fn visit_item_struct(&mut self, struct_def: &'ast syn::ItemStruct) {
        let name = struct_def.ident.to_string();
        let mut fields = HashMap::new();
        let location = format!(
            "Definition: {}:{}",
            self.file
                .display()
                .to_string()
                .replace(&self.base_dir.display().to_string(), ""),
            struct_def.struct_token.span().start().line
        );
        for field in &struct_def.fields {
            if let Some(type_rep) = TypeRepr::from_syn_type(&self.crate_name, &field.ty) {
                fields
                    .entry(field.ident.as_ref().unwrap().to_string())
                    .or_insert(type_rep);
            }
        }
        self.structs.push(StructDefinition {
            name,
            fields,
            location,
            file: self.file.clone(),
            crate_name: self.crate_name.clone(),
        });
    }

    fn visit_item_fn(&mut self, fn_def: &'ast syn::ItemFn) {
        if Self::has_tauri_command_attr(&fn_def.attrs) {
            let name = fn_def.sig.ident.to_string();

            let mut param_names = Vec::new();
            let mut params = HashMap::new();
            for input in &fn_def.sig.inputs {
                if let syn::FnArg::Typed(pat_type) = input
                    && let syn::Pat::Ident(id) = pat_type.pat.as_ref()
                    && !Self::is_type_excluded(&pat_type.ty)
                {
                    let name = id.ident.to_string();
                    param_names.push(name.clone());
                    if let Some(par_ty) = TypeRepr::from_syn_type("", &pat_type.ty) {
                        params.entry(name).or_insert(par_ty);
                    }
                }
            }

            let mut ret_type = None;
            if let syn::ReturnType::Type(_, ty) = &fn_def.sig.output {
                if let Some(ret_typ) = TypeRepr::from_syn_type("", ty.as_ref()) {
                    ret_type = Some(ret_typ);
                }
            }
            let location = format!(
                "Definition: {}:{}",
                self.file
                    .display()
                    .to_string()
                    .replace(&self.base_dir.display().to_string(), ""),
                fn_def.sig.span().start().line
            );

            self.commands.push(CommandDefinition {
                name,
                ret_type,
                params,
                param_names,
                file: self.file.clone(),
                location,
            });
        }

        syn::visit::visit_item_fn(self, fn_def);
    }

    // Detecta parámetros de función: fn foo(app: AppHandle, ...)
    fn visit_fn_arg(&mut self, node: &'ast FnArg) {
        if let FnArg::Typed(pat_type) = node {
            if let Some(name) = Self::extract_pat_ident(&pat_type.pat) {
                if Self::is_app_type(&pat_type.ty) {
                    self.app_handle_vars.insert(name);
                } else if Self::type_as_path_string(&pat_type.ty).is_some() {
                    if let Some(ty) = self.get_type_repr(&pat_type.ty) {
                        self.vars.insert(name, ty);
                    }
                }
            }
        }
        syn::visit::visit_fn_arg(self, node);
    }

    // Detecta declaraciones locales con y sin tipo explícito
    fn visit_local(&mut self, node: &'ast Local) {
        match &node.pat {
            // con tipo explícito: let app: AppHandle = ...
            Pat::Type(PatType { ty, pat, .. }) => {
                if let Some(name) = Self::extract_pat_ident(pat) {
                    if Self::is_app_type(ty) {
                        self.app_handle_vars.insert(name);
                    } else if Self::type_as_path_string(ty).is_some() {
                        if let Some(ty) = self.get_type_repr(ty) {
                            self.vars.insert(name, ty);
                        }
                    }
                }
            }
            // sin tipo explícito: let handle2 = handle1 o let x = expr
            Pat::Ident(pat_ident) => {
                let name = pat_ident.ident.to_string();
                if let Some(init) = &node.init {
                    let init_repr = init.expr.to_token_stream().to_string();
                    // propagar AppHandle: let handle2 = handle1
                    if self.app_handle_vars.contains(&init_repr) {
                        self.app_handle_vars.insert(name);
                    }
                    // para otros tipos sin anotación no hay TypeRepr que inferir
                }
            }
            _ => {}
        }
        syn::visit::visit_local(self, node);
    }

    // Parámetros de closure: |app: &mut tauri::App| { ... }
    fn visit_expr_closure(&mut self, node: &'ast ExprClosure) {
        for input in &node.inputs {
            if let Pat::Type(PatType { ty, pat, .. }) = input {
                if Self::is_app_type(ty) {
                    if let Some(name) = Self::extract_pat_ident(pat) {
                        self.app_handle_vars.insert(name);
                    }
                }
            }
        }
        syn::visit::visit_expr_closure(self, node);
    }

    // Detecta llamadas a .emit*()
    fn visit_expr_method_call(&mut self, node: &'ast ExprMethodCall) {
        if node.method.to_string().starts_with("emit") {
            let receiver = node.receiver.to_token_stream().to_string();
            if self.app_handle_vars.contains(&receiver) {
                let args: Vec<String> = node
                    .args
                    .iter()
                    .map(|arg| arg.to_token_stream().to_string().replace("\"", ""))
                    .collect();

                match node.method.to_string().as_str() {
                    "emit" => {
                        self.events.push(EventDefinition {
                            name: args.get(0).unwrap().replace('"', ""),
                            ty: self.vars[args.get(1).unwrap()].clone(),
                            file: self.file.clone(),
                        });
                    }
                    "emit_to" => {
                        self.events.push(EventDefinition {
                            name: args.get(1).unwrap().replace('"', ""),
                            ty: self.vars[args.get(2).unwrap()].clone(),
                            file: self.file.clone(),
                        });
                    }
                    "emit_str" => {
                        self.events.push(EventDefinition {
                            name: args.get(0).unwrap().replace('"', ""),
                            ty: TypeRepr::Simple("".to_string(), "String".to_string()),
                            file: self.file.clone(),
                        });
                    }
                    "emit_str_to" => {
                        self.events.push(EventDefinition {
                            name: args.get(1).unwrap().replace('"', ""),
                            ty: TypeRepr::Simple("".to_string(), "String".to_string()),
                            file: self.file.clone(),
                        });
                    }
                    _ => (),
                };
            }
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}
