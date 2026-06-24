use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::exit,
};

use syn::spanned::Spanned;

use crate::commons::{TypeRepr, collect_imports, standard_type_assoc};

#[derive(Debug, Clone)]
pub struct CommandDefinition {
    pub name: String,
    pub ret_type: Option<TypeRepr>,
    pub param_names: Vec<String>,
    pub params: HashMap<String, TypeRepr>,
    pub location: String,
    pub file: PathBuf,
}

impl CommandDefinition {
    pub fn find<F, B>(file: F, base_dir: B) -> Vec<CommandDefinition>
    where
        F: AsRef<Path>,
        B: AsRef<Path>,
    {
        let content = fs::read_to_string(file.as_ref()).unwrap();
        let file_syn = syn::parse_file(&content);
        if file_syn.is_err() {
            exit(0);
        }
        let file_syn = file_syn.unwrap();
        let items = file_syn.items;

        let mut res = Vec::new();

        for item in items {
            if let syn::Item::Fn(fn_def) = item
                && Self::has_tauri_command_attr(&fn_def.attrs)
            {
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
                if let syn::ReturnType::Type(_, ty) = &fn_def.sig.output
                    && let Some(ret_typ) = TypeRepr::from_syn_type("", ty.as_ref())
                {
                    ret_type = Some(ret_typ);
                }
                let location = format!(
                    "Definition: {}:{}",
                    file.as_ref()
                        .display()
                        .to_string()
                        .replace(&base_dir.as_ref().display().to_string(), ""),
                    fn_def.sig.span().start().line
                );

                res.push(Self {
                    name,
                    ret_type,
                    params,
                    param_names,
                    file: file.as_ref().to_path_buf(),
                    location,
                });
            }
        }

        res
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

    pub fn get_inner_leafs(&self) -> Vec<String> {
        let imports = collect_imports(&self.file);

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
