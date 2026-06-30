use std::{
    collections::HashSet,
    fs,
    hash::Hash,
    path::{Path, PathBuf},
};

use syn::spanned::Spanned;

#[derive(Debug, Clone)]
pub struct EnumDefinition {
    pub name: String,
    pub variants: Vec<String>,
    pub location: String,
    pub file: PathBuf,
    pub syn_file: syn::File,
    pub crate_name: String,
    pub imports: HashSet<String>,
}

impl Eq for EnumDefinition {}

impl PartialEq for EnumDefinition {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.crate_name == other.crate_name
    }
}

impl Hash for EnumDefinition {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
        self.crate_name.hash(state);
    }
}

impl EnumDefinition {
    pub fn get_full_qualified_name(&self) -> String {
        format!("{}::{}", self.crate_name, self.name)
    }

    pub fn to_typescript(&self) -> String {
        let mut code = String::new();
        code.push_str(&format!("// {}\n", &self.location));
        code.push_str(&format!("export enum {} {{\n", self.name));
        for variant in &self.variants {
            code.push_str(&format!("\t{} = \"{}\",\n", variant, variant));
        }
        code.push('}');
        code
    }

    pub fn generate_file<F>(file: F, enums: &Vec<EnumDefinition>)
    where
        F: AsRef<Path>,
    {
        let mut content = fs::read_to_string(&file).unwrap();
        for enum_d in enums {
            content.push_str(&enum_d.to_typescript());
            content.push_str("\n\n");
        }

        fs::write(&file, content).unwrap();
    }

    pub fn from_item_struct(
        item_enum: &syn::ItemEnum,
        base_dir: &PathBuf,
        file: &PathBuf,
        syn_file: &syn::File,
        crate_name: &str,
        imports: &HashSet<String>,
    ) -> Self {
        let name = item_enum.ident.to_string();
        let variants = item_enum
            .variants
            .iter()
            .map(|v| v.ident.to_string())
            .collect::<Vec<String>>();

        let location = format!(
            "Definition: {}:{}",
            file.display()
                .to_string()
                .replace(&base_dir.display().to_string(), ""),
            item_enum.enum_token.span().start().line
        );
        EnumDefinition {
            name,
            variants,
            location,
            file: file.clone(),
            syn_file: syn_file.clone(),
            crate_name: crate_name.to_string(),
            imports: imports.clone(),
        }
    }
}
