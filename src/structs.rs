use std::{
    collections::{HashMap, HashSet},
    fs,
    hash::Hash,
    path::{Path, PathBuf},
};

use syn::spanned::Spanned;

use crate::commons::{standard_type_assoc, TypeRepr};

#[derive(Debug, Clone)]
pub struct StructDefinition {
    pub name: String,
    pub fields: HashMap<String, TypeRepr>,
    pub location: String,
    pub file: PathBuf,
    pub syn_file: syn::File,
    pub crate_name: String,
    pub imports: HashSet<String>,
}

impl Eq for StructDefinition {}

impl PartialEq for StructDefinition {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.crate_name == other.crate_name
    }
}

impl Hash for StructDefinition {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.crate_name.hash(state);
    }
}

impl StructDefinition {
    pub fn get_full_qualified_name(&self) -> String {
        format!("{}::{}", self.crate_name, self.name)
    }

    pub fn to_typescript(&self) -> String {
        let mut code = String::new();

        code.push_str(&format!("// {}\n", &self.location));
        code.push_str(&format!("export interface {} {{\n", self.name));

        let mut keys = self.fields.keys().cloned().collect::<Vec<String>>();
        keys.sort();
        for key in keys {
            code.push_str(&format!(
                "  {}: {};\n",
                key,
                self.fields.get(&key).unwrap().to_typescript()
            ));
        }

        code.push('}');

        code
    }

    pub fn get_inner_leafs(&self) -> Vec<String> {
        let mut types = Vec::new();
        for field in &self.fields {
            types.push(field.1);
        }

        let mut res = Vec::new();
        for ty in types {
            for ty2 in ty.inner_leaf_types() {
                if standard_type_assoc(&ty2).is_none() {
                    let fallback_path = format!("{}::{}", self.crate_name, ty2).to_string();
                    let path = self
                        .imports
                        .iter()
                        .find(|i| i.ends_with(&ty2))
                        .unwrap_or(&fallback_path);
                    if !res.contains(path) {
                        res.push(path.clone());
                    }
                }
            }
        }

        res
    }

    pub fn generate_file<F>(file: F, structs: &Vec<StructDefinition>)
    where
        F: AsRef<Path>,
    {
        if fs::exists(&file).unwrap() {
            fs::remove_file(&file).unwrap();
        }

        let mut content = String::new();
        for struct_d in structs {
            content.push_str(&struct_d.to_typescript());
            content.push_str("\n\n");
        }

        fs::write(&file, content).unwrap();
    }

    pub fn from_item_struct(
        struct_def: &syn::ItemStruct,
        base_dir: &PathBuf,
        file: &PathBuf,
        syn_file: &syn::File,
        crate_name: &str,
        imports: &HashSet<String>,
    ) -> Self {
        let name = struct_def.ident.to_string();
        let mut fields = HashMap::new();
        let location = format!(
            "Definition: {}:{}",
            file.display()
                .to_string()
                .replace(&base_dir.display().to_string(), ""),
            struct_def.struct_token.span().start().line
        );
        for field in &struct_def.fields {
            if let Some(type_rep) = TypeRepr::from_syn_type(crate_name, &field.ty) {
                fields
                    .entry(field.ident.as_ref().unwrap().to_string())
                    .or_insert(type_rep);
            }
        }
        StructDefinition {
            name,
            fields,
            location,
            file: file.clone(),
            syn_file: syn_file.clone(),
            crate_name: crate_name.to_string(),
            imports: imports.clone(),
        }
    }
}
