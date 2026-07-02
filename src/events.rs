use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use quote::ToTokens;
use syn::{Expr, ExprMethodCall, spanned::Spanned};

use crate::{
    RA_CLIENT,
    commons::{TypeRepr, standard_type_assoc},
};

#[derive(Debug, Clone)]
pub struct EventDefinition {
    pub name: String,
    pub ty: TypeRepr,
    pub file: PathBuf,
    pub syn_file: syn::File,
    pub imports: HashSet<String>,
}

impl EventDefinition {
    pub fn get_inner_leafs(&self) -> Vec<String> {
        let mut res = Vec::new();
        for ty in self.ty.inner_leaf_types() {
            let path = self
                .imports
                .iter()
                .find(|i| i.ends_with(&ty))
                .unwrap_or(&ty);
            res.push(path.clone());
        }

        res
    }

    fn to_typescript(&self) -> String {
        let mut res = String::new();
        res.push_str(&format!(
            "public static on{}(callback: (payload: {}) => void): () => void {{\n",
            self.name_to_pascalcase(),
            self.ty.to_typescript()
        ));

        res.push_str(&format!(
            "  return BackendListener.inner_listen<{}>(\"{}\", callback);\n",
            self.ty.to_typescript(),
            self.name
        ));

        res.push('}');

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

    pub fn generate_file<T>(file: T, events: &Vec<EventDefinition>)
    where
        T: AsRef<Path>,
    {
        if fs::exists(&file).unwrap() {
            fs::remove_file(&file).unwrap();
        }

        let mut struct_names = HashSet::new();
        for event in events {
            for ty in event.get_inner_leafs() {
                if let Some(name) = ty.split("::").last()
                    && standard_type_assoc(name).is_none()
                {
                    struct_names.insert(name.to_string());
                }
            }
        }
        let mut struct_names = struct_names.iter().cloned().collect::<Vec<_>>();
        struct_names.sort();
        let struct_names = struct_names.join(", ");

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

    pub fn get_type_from_expr(expr: &Expr, file: &PathBuf) -> Option<TypeRepr> {
        let start = expr.span().start();

        match RA_CLIENT.lock().unwrap().as_mut().unwrap().type_at(
            file,
            start.line,
            start.column + 1,
        ) {
            Ok(Some(t)) => TypeRepr::from_type_str("", t.type_string.unwrap().as_str()),
            Ok(None) => {
                dbg!(&file);
                dbg!(&start);
                None
            }
            Err(e) => {
                dbg!(&file);
                dbg!(&start);
                dbg!(e);
                None
            }
        }
    }

    pub fn get_offset(content: &str, line: usize, column: usize) -> usize {
        let mut offset = 0;

        for (idx, current_line) in content.lines().enumerate() {
            if idx + 1 == line {
                offset += column;
                break;
            }

            offset += current_line.len() + 1;
        }

        offset
    }

    pub fn from_expr_method_call(
        node: &ExprMethodCall,
        app_handle_vars: &HashSet<String>,
        imports: &HashSet<String>,
        file: &PathBuf,
        syn_file: &syn::File,
    ) -> Option<Self> {
        if node.method.to_string().starts_with("emit") {
            let receiver = node.receiver.to_token_stream().to_string();
            if app_handle_vars.contains(&receiver) {
                let args_str: Vec<String> = node
                    .args
                    .iter()
                    .map(|arg| arg.to_token_stream().to_string().replace("\"", ""))
                    .collect();

                let arg_expr: Vec<Expr> = node.args.iter().cloned().collect();

                let payload_idx = if node.method.to_string().as_str().ends_with("_to") {
                    2
                } else {
                    1
                };
                let ty =
                    Self::get_type_from_expr(arg_expr.get(payload_idx).unwrap(), file).unwrap();

                match node.method.to_string().as_str() {
                    "emit" => Some(EventDefinition {
                        name: args_str.first().unwrap().replace('"', ""),
                        ty,
                        file: file.clone(),
                        syn_file: syn_file.clone(),
                        imports: imports.clone(),
                    }),
                    "emit_to" => Some(EventDefinition {
                        name: args_str.get(1).unwrap().replace('"', ""),
                        ty,
                        file: file.clone(),
                        syn_file: syn_file.clone(),
                        imports: imports.clone(),
                    }),
                    "emit_str" => Some(EventDefinition {
                        name: args_str.first().unwrap().replace('"', ""),
                        ty,
                        file: file.clone(),
                        syn_file: syn_file.clone(),
                        imports: imports.clone(),
                    }),
                    "emit_str_to" => Some(EventDefinition {
                        name: args_str.get(1).unwrap().replace('"', ""),
                        ty,
                        file: file.clone(),
                        syn_file: syn_file.clone(),
                        imports: imports.clone(),
                    }),
                    _ => None,
                }
            } else {
                None
            }
        } else {
            None
        }
    }
}
