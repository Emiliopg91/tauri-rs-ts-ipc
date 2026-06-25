use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
};

use crate::commons::{TypeRepr, standard_type_assoc};

#[derive(Debug, Clone)]
pub struct EventDefinition {
    pub name: String,
    pub ty: TypeRepr,
    pub file: PathBuf,
    pub syn_file: syn::File,
    pub imports: Vec<String>
}

impl EventDefinition {
    pub fn get_inner_leafs(&self) -> Vec<String> {
        let mut res = Vec::new();
        for ty in self.ty.inner_leaf_types() {
            let path = self.imports.iter().find(|i| i.ends_with(&ty)).unwrap_or(&ty);
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
