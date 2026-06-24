use std::{
    collections::HashMap,
    fs,
    hash::Hash,
    path::{Path, PathBuf},
};

use syn::spanned::Spanned;

use crate::commons::{TypeRepr, collect_imports, standard_type_assoc};

#[derive(Debug, Clone)]
pub struct StructDefinition {
    pub name: String,
    pub fields: HashMap<String, TypeRepr>,
    pub location: String,
    pub file: PathBuf,
    pub crate_name: String,
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

    pub fn find<F, B>(file: F, base_dir: B) -> HashMap<String, StructDefinition>
    where
        F: AsRef<Path>,
        B: AsRef<Path>,
    {
        let mut crate_name = file
            .as_ref()
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

        println!("{}", file.as_ref().display());
        let content = fs::read_to_string(file.as_ref()).unwrap();
        let file_syn = syn::parse_file(&content).unwrap();
        let items = &file_syn.items;

        let mut res = HashMap::new();
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
                    if let Some(type_rep) = TypeRepr::from_syn_type(&crate_name, &field.ty) {
                        fields
                            .entry(field.ident.as_ref().unwrap().to_string())
                            .or_insert(type_rep);
                    }
                }
                res.entry(name.clone()).or_insert(Self {
                    name,
                    fields,
                    location,
                    file: file.as_ref().to_path_buf(),
                    crate_name: crate_name.clone(),
                });
            }
        }

        res
    }

    pub fn to_typescript(&self) -> String {
        let mut code = String::new();

        code.push_str(&format!("// {}\n", &self.location));
        code.push_str(&format!("export interface {} {{\n", self.name));

        for field in &self.fields {
            code.push_str(&format!("  {}: {};\n", field.0, field.1.to_typescript()));
        }

        code.push('}');

        code
    }

    pub fn get_inner_leafs(&self) -> Vec<String> {
        let imports = collect_imports(&self.file);

        let mut types = Vec::new();
        for field in &self.fields {
            types.push(field.1);
        }

        let mut res = Vec::new();
        for ty in types {
            for ty2 in ty.inner_leaf_types() {
                if standard_type_assoc(&ty2).is_none() {
                    let fallback_path = format!("{}::{}", self.crate_name, ty2).to_string();
                    let path = imports
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

    pub fn generate_file<F>(file: F, structs: Vec<StructDefinition>)
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
}
