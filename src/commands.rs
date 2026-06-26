use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use syn::spanned::Spanned;

use crate::commons::{TypeRepr,  standard_type_assoc};

#[derive(Debug, Clone)]
pub struct CommandDefinition {
    pub name: String,
    pub ret_type: Option<TypeRepr>,
    pub param_names: Vec<String>,
    pub params: HashMap<String, TypeRepr>,
    pub location: String,
    pub file: PathBuf,
    pub syn_file: syn::File,
    pub imports: HashSet<String>
}

impl CommandDefinition {
    pub fn get_inner_leafs(&self) -> Vec<String> {
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
                let path = self.imports.iter().find(|i| i.ends_with(&ty2)).unwrap_or(&ty2);
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

    pub fn generate_file<T>(file: T, commands:& Vec<CommandDefinition>)
    where
        T: AsRef<Path>,
    {
        if fs::exists(&file).unwrap() {
            fs::remove_file(&file).unwrap();
        }

        let mut struct_names = HashSet::new();
        for cmd in commands {
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

    fn has_tauri_command_attr(attrs: &[syn::Attribute]) -> bool {
        attrs.iter().any(|attr| {
            let path = attr.path();
            path.segments.len() == 2
                && path.segments[0].ident == "tauri"
                && path.segments[1].ident == "command"
        })
    }

    fn is_type_excluded(ty: &syn::Type) -> bool {
        for ity in &["AppHandle", "State", "Channel"]{
            if quote::quote!(#ty).to_string() == ity.to_string() ||  quote::quote!(#ty).to_string().ends_with(&format!("::{}", ity)){
                return true;
            }
        }

        false
    }

    pub fn from_item_fn(fn_def: &syn::ItemFn,imports : &HashSet<String>, base_dir: &PathBuf, file: &PathBuf, syn_file: &syn::File)-> Option<Self>{
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
            if let syn::ReturnType::Type(_, ty) = &fn_def.sig.output
                && let Some(ret_typ) = TypeRepr::from_syn_type("", ty.as_ref())
            {
                ret_type = Some(ret_typ);
            }
            let location = format!(
                "Definition: {}:{}",
                file
                    .display()
                    .to_string()
                    .replace(&base_dir.display().to_string(), ""),
                fn_def.sig.span().start().line
            );

            Some(CommandDefinition {
                name,
                ret_type,
                params,
                param_names,
                file: file.clone(),
                syn_file: syn_file.clone(),
                location,
                imports:imports.clone()
            })
        } else {
            None
        }
    }
    
}
