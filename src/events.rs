use quote::ToTokens;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::exit,
};
use syn::{ExprClosure, ExprMethodCall, FnArg, Local, Pat, PatType, Type, visit::Visit};

use crate::commons::{TypeRepr, collect_imports, standard_type_assoc};

struct EmitFinder {
    app_handle_vars: HashSet<String>,
    vars: HashMap<String, TypeRepr>,
    events: HashMap<String, TypeRepr>,
    imports: Vec<String>,
}

impl EmitFinder {
    fn new<F>(file: F) -> Self
    where
        F: AsRef<Path>,
    {
        Self {
            app_handle_vars: HashSet::new(),
            vars: HashMap::new(),
            events: HashMap::new(),
            imports: collect_imports(file),
        }
    }

    fn is_app_type(ty: &syn::Type) -> bool {
        ty.to_token_stream().to_string().contains("AppHandle")
            || ty.to_token_stream().to_string().contains("App")
    }

    fn extract_pat_ident(pat: &Pat) -> Option<String> {
        match pat {
            Pat::Ident(ident) => Some(ident.ident.to_string()),
            _ => None,
        }
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
}

impl<'ast> Visit<'ast> for EmitFinder {
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

                println!("{} -> {:?}", node.method.to_string(), args);
                println!("{:?}", self.vars);
                match node.method.to_string().as_str() {
                    "emit" => {
                        self.events.insert(
                            args.get(0).unwrap().replace('"', ""),
                            self.vars[args.get(1).unwrap()].clone(),
                        );
                    }
                    "emit_to" => {
                        self.events.insert(
                            args.get(1).unwrap().replace('"', ""),
                            self.vars[args.get(2).unwrap()].clone(),
                        );
                    }
                    "emit_str" => {
                        self.events.insert(
                            args.get(0).unwrap().replace('"', ""),
                            TypeRepr::Simple("".to_string(), "String".to_string()),
                        );
                    }
                    "emit_str_to" => {
                        self.events.insert(
                            args.get(1).unwrap().replace('"', ""),
                            TypeRepr::Simple("".to_string(), "String".to_string()),
                        );
                    }
                    _ => (),
                };
            }
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

#[derive(Debug)]
pub struct EventDefinition {
    pub name: String,
    pub ty: TypeRepr,
    pub file: PathBuf,
}

impl EventDefinition {
    pub fn find<F, B>(file: F, _base_dir: B) -> Vec<EventDefinition>
    where
        F: AsRef<Path>,
        B: AsRef<Path>,
    {
        let content = fs::read_to_string(file.as_ref()).unwrap();
        let file_syn = syn::parse_file(&content).unwrap();

        let mut finder = EmitFinder::new(&file);
        finder.visit_file(&file_syn);

        let mut res = Vec::new();

        for event in finder.events {
            res.push(EventDefinition {
                name: event.0,
                ty: event.1,
                file: file.as_ref().to_path_buf(),
            })
        }

        res
    }

    pub fn get_inner_leafs(&self) -> Vec<String> {
        let mut res = Vec::new();
        let imports = collect_imports(&self.file);
        for ty in self.ty.inner_leaf_types() {
            let path = imports.iter().find(|i| i.ends_with(&ty)).unwrap_or(&ty);
            res.push(path.clone());
        }

        res
    }

    fn to_typescript(&self) -> String {
        let mut res = String::new();
        res.push_str(&format!(
            "public static on{}(callback: (payload: {}) => void) {{\n",
            self.name_to_pascalcase(),
            self.ty.to_typescript()
        ));

        res.push_str(&format!(
            "  BackendListener.inner_listen<{}>(\"{}\", callback);\n",
            self.ty.to_typescript(),
            self.name
        ));

        res.push_str("}");

        res
    }

    fn name_to_pascalcase(&self) -> String {
        let mut res = String::new();
        let mut upper = true;
        let chars = self.name.chars();
        for c in chars {
            if c == '_' {
                upper = true;
            } else {
                if upper {
                    res.extend(c.to_uppercase());
                } else {
                    res.push(c);
                }
                upper = false;
            }
        }

        res
    }

    pub fn generate_file<T>(file: T, events: Vec<EventDefinition>)
    where
        T: AsRef<Path>,
    {
        if fs::exists(&file).unwrap() {
            fs::remove_file(&file).unwrap();
        }

        let mut struct_names = HashSet::new();
        for event in &events {
            for ty in event.get_inner_leafs() {
                if let Some(name) = ty.split("::").last()
                    && standard_type_assoc(name).is_none()
                {
                    struct_names.insert(name.to_string());
                }
            }
        }
        let struct_names = struct_names.iter().cloned().collect::<Vec<_>>().join(", ");

        let mut content = String::new();
        content.push_str(
            "//Auto generated file, do not edit manually

import { listen } from \"@tauri-apps/api/event\";\n\n",
        );
        content.push_str(&format!(
            "import {{ {} }} from \"./models\";\n\n",
            struct_names
        ));

        content.push_str("export class BackendListener {\n");
        for event in events {
            content.push_str(&format!(
                "\t{}",
                &event.to_typescript().replace("\n", "\n\t")
            ));
            content.push_str("\n\n");
        }
        content.push_str(
            "  private static inner_listen<R>(event_name: string, callback: (payload: R) => void ): () => void {
    const unlisten = listen<R>(event_name, (event) => {
      callback(event.payload);
    });

    return () => { unlisten.then((fn) => fn()); };
  }\n",
        );
        content.push('}');

        fs::write(&file, content).unwrap();
    }
}
