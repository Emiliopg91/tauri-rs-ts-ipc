use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use crate::commons::{TypeRepr, collect_imports, standard_type_assoc};

#[derive(Debug, Clone)]
pub struct CommandDefinition {
    pub name: String,
    pub ret_type: Option<TypeRepr>,
    pub param_names: Vec<String>,
    pub params: HashMap<String, TypeRepr>,
    pub location: String,
    pub file: PathBuf,
    pub syn_file: syn::File,
}

impl CommandDefinition {
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

    pub fn get_inner_leafs(&self) -> Vec<String> {
        let imports = collect_imports(&self.syn_file);

        let mut types = Vec::new();
        for param in &self.params {
            types.push(param.1);
        }
        if let Some(ret_type) = &self.ret_type {
            types.push(ret_type);
        }

        let mut res = Vec::new();
        for ty in types {
            for ty2 in ty.inner_leaf_types() {
                let path = imports.iter().find(|i| i.ends_with(&ty2)).unwrap_or(&ty2);
                res.push(path.clone());
            }
        }

        res
    }

    pub fn to_typescript(&self) -> String {
        let mut res = String::new();
        let ret_type_str = match &self.ret_type {
            Some(v) => v.to_typescript(),
            None => "void".to_string(),
        };

        let mut pars = Vec::new();
        for name in &self.param_names {
            let definition = self.params.get(name).unwrap();
            pars.push((name, definition));
        }
        let pars = pars
            .iter()
            .map(|p| format!("{}: {}", p.0, p.1.to_typescript()))
            .collect::<Vec<String>>()
            .join(", ");

        let mut payload = "".to_string();
        if !self.params.is_empty() {
            payload = format!(", {{ {} }}", self.param_names.join(", "));
        }

        res.push_str(&format!("// {}\n", &self.location));
        res.push_str(&format!(
            "public static {}({}): Promise<{}> {{\n",
            self.name_to_camelcase(),
            pars,
            ret_type_str
        ));
        res.push_str(&format!(
            "  return BackendClient.inner_invoke(\"{}\"{}); \n",
            &self.name, payload
        ));
        res.push_str("}\n");

        res
    }

    fn name_to_camelcase(&self) -> String {
        let mut res = String::new();
        let mut upper = false;
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

    pub fn generate_file<T>(file: T, commands: Vec<CommandDefinition>)
    where
        T: AsRef<Path>,
    {
        if fs::exists(&file).unwrap() {
            fs::remove_file(&file).unwrap();
        }

        let mut struct_names = HashSet::new();
        for cmd in &commands {
            for ty in cmd.get_inner_leafs() {
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

import { invoke, InvokeArgs } from \"@tauri-apps/api/core\";\n\n",
        );
        content.push_str(&format!(
            "import {{ {} }} from \"./models\";\n\n",
            struct_names
        ));
        content.push_str("export class BackendClient {\n");
        for cmd in commands {
            content.push_str(&format!("\t{}", &cmd.to_typescript().replace("\n", "\n\t")));
            content.push_str("\n\n");
        }
        content.push_str(
            "	private static inner_invoke<R>(method: string, payload?: InvokeArgs): Promise<R> {
		return invoke(method, payload);
	}\n",
        );
        content.push('}');

        fs::write(&file, content).unwrap();
    }
}
