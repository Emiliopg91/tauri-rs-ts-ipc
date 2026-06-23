use std::{fs, path::Path};

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
