pub mod commands;
pub mod commons;
pub mod events;
pub mod structs;

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use syn::visit::Visit;

use crate::{
    commands::CommandDefinition, commons::RsTsVisitor, events::EventDefinition,
    structs::StructDefinition,
};

fn find_rs_files<T>(path: T, out: &mut Vec<PathBuf>) -> Result<(), std::io::Error>
where
    T: AsRef<Path>,
{
    for entry in fs::read_dir(path.as_ref())?.flatten() {
        let entry = entry.path();
        if entry.is_file() {
            if entry.extension().unwrap() == "rs" {
                out.push(entry);
            }
        } else if entry.is_dir() {
            find_rs_files(entry, out)?;
        }
    }

    Ok(())
}

pub fn build() {
    let src_tauri_path_buf = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src_tauri_path = src_tauri_path_buf.as_path().join("src");
    let project_dir = src_tauri_path_buf.as_path().parent().unwrap().to_path_buf();
    let backend_dir = project_dir.join("src").join("utils").join("backend");
    let models_path = PathBuf::from(backend_dir.join("models.ts").display().to_string());
    let client_path = PathBuf::from(backend_dir.join("client.ts").display().to_string());
    let listener_path = PathBuf::from(backend_dir.join("listener.ts").display().to_string());

    inner_build(
        &project_dir,
        &backend_dir,
        &src_tauri_path,
        &models_path,
        &client_path,
        &listener_path,
    );
}

pub fn inner_build(
    project_dir: &PathBuf,
    backend_dir: &PathBuf,
    src_tauri_path: &PathBuf,
    models_path: &PathBuf,
    client_path: &PathBuf,
    listener_path: &PathBuf,
) {
    println!("cargo:warning=Gathering Rust source codes...");
    let mut files = Vec::new();
    find_rs_files(&src_tauri_path, &mut files).unwrap();

    let mut commands = Vec::new();
    let mut events = Vec::new();
    let mut structs = Vec::new();

    println!("cargo:warning=Inspecting source code...");
    for file in &files {
        let content = fs::read_to_string(file).unwrap();
        let file_syn = syn::parse_file(&content).unwrap();
        let mut finder = RsTsVisitor::new(file, src_tauri_path);
        finder.visit_file(&file_syn);

        for cmd in finder.commands {
            commands.push(cmd);
        }

        for event in finder.events {
            events.push(event);
        }

        for struct_d in finder.structs {
            structs.push(struct_d);
        }
    }
    println!("cargo:warning=  Found {} commands", commands.len());
    println!("cargo:warning=  Found {} events", events.len());
    println!("cargo:warning=  Found {} structs", structs.len());

    let mut files_for_structs: HashMap<String, Vec<String>> = HashMap::new();
    for cmd in &commands {
        for pat in cmd.get_inner_leafs() {
            let struct_name = pat.split("::").last().unwrap_or(&pat);

            if let Some(path) = pat.strip_prefix("crate::") {
                let path = PathBuf::from(&src_tauri_path)
                    .join(path.replace("::", "/"))
                    .parent()
                    .unwrap()
                    .to_path_buf();
                let mut path_str = format!("{}.rs", path.display());
                if !fs::exists(&path_str).unwrap() {
                    path_str = format!("{}/mod.rs", path.display());
                }

                let entry = files_for_structs.entry(path_str).or_default();
                entry.push(struct_name.to_string());
            }
        }
    }
    for event in &events {
        for pat in event.get_inner_leafs() {
            let struct_name = pat.split("::").last().unwrap_or(&pat);

            if let Some(path) = pat.strip_prefix("crate::") {
                let path = PathBuf::from(&src_tauri_path)
                    .join(path.replace("::", "/"))
                    .parent()
                    .unwrap()
                    .to_path_buf();
                let mut path_str = format!("{}.rs", path.display());
                if !fs::exists(&path_str).unwrap() {
                    path_str = format!("{}/mod.rs", path.display());
                }

                let entry = files_for_structs.entry(path_str).or_default();
                entry.push(struct_name.to_string());
            }
        }
    }

    let mut checked_structs = Vec::new();
    let mut already_added = HashSet::new();
    loop {
        let mut new_structs = Vec::new();
        for (file, structs_to_find) in &files_for_structs {
            let content = fs::read_to_string(file).unwrap();
            let file_syn = syn::parse_file(&content).unwrap();
            let mut finder = RsTsVisitor::new(file, src_tauri_path);
            finder.visit_file(&file_syn);

            for struct_d in finder.structs {
                if structs_to_find.contains(&struct_d.name)
                    && already_added.insert(struct_d.name.clone())
                {
                    new_structs.push(struct_d);
                }
            }
        }

        files_for_structs.clear();
        for struct_d in &new_structs {
            for pat in struct_d.get_inner_leafs() {
                let struct_name = pat.split("::").last().unwrap_or(&pat);

                if let Some(path) = pat.strip_prefix("crate::") {
                    let path = PathBuf::from(&src_tauri_path)
                        .join(path.replace("::", "/"))
                        .parent()
                        .unwrap()
                        .to_path_buf();
                    let mut path_str = format!("{}.rs", path.display());
                    if !fs::exists(&path_str).unwrap() {
                        path_str = format!("{}/mod.rs", path.display());
                    }

                    let entry = files_for_structs.entry(path_str).or_default();
                    entry.push(struct_name.to_string());
                }
            }
        }

        for ns in new_structs {
            checked_structs.push(ns);
        }
        if files_for_structs.is_empty() {
            break;
        }
    }
    println!("cargo:warning=  Found {} structs", checked_structs.len());

    println!(
        "cargo:warning=Generating models file in '{}'",
        models_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
            .to_string()
    );
    checked_structs.sort_by_key(|e| e.name.clone());
    StructDefinition::generate_file(models_path, checked_structs);

    println!(
        "cargo:warning=Generating client file in '{}'",
        client_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
            .to_string()
    );
    commands.sort_by_key(|e| e.name.clone());
    CommandDefinition::generate_file(client_path, commands);

    println!(
        "cargo:warning=Generating listener file in '{}'",
        listener_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
            .to_string()
    );
    events.sort_by_key(|e| e.name.clone());
    EventDefinition::generate_file(listener_path, events);

    /*
    if !fs::exists(&backend_dir).unwrap() {
        fs::create_dir_all(&backend_dir).unwrap();
    }

    let mut files = Vec::new();
    find_rs_files(&src_tauri_path, &mut files).unwrap();

    let mut commands = Vec::new();
    for file in &files {
        for cmd in CommandDefinition::find(file, &src_tauri_path) {
            commands.push(cmd);
        }
    }

    println!("cargo:warning=Looking for events...");
    let mut events = Vec::new();
    for file in &files {
        for event in EventDefinition::find(file, &src_tauri_path) {
            events.push(event);
        }
    }
    println!("cargo:warning=  Found {} events", events.len());

    println!("cargo:warning=Looking for structs...");
    let mut files_for_structs: HashMap<String, Vec<String>> = HashMap::new();
    for cmd in &commands {
        for pat in cmd.get_inner_leafs() {
            let struct_name = pat.split("::").last().unwrap_or(&pat);

            if let Some(path) = pat.strip_prefix("crate::") {
                let path = PathBuf::from(&src_tauri_path)
                    .join(path.replace("::", "/"))
                    .parent()
                    .unwrap()
                    .to_path_buf();
                let mut path_str = format!("{}.rs", path.display());
                if !fs::exists(&path_str).unwrap() {
                    path_str = format!("{}/mod.rs", path.display());
                }

                let entry = files_for_structs.entry(path_str).or_default();
                entry.push(struct_name.to_string());
            }
        }
    }
    for event in &events {
        for pat in event.get_inner_leafs() {
            let struct_name = pat.split("::").last().unwrap_or(&pat);

            if let Some(path) = pat.strip_prefix("crate::") {
                let path = PathBuf::from(&src_tauri_path)
                    .join(path.replace("::", "/"))
                    .parent()
                    .unwrap()
                    .to_path_buf();
                let mut path_str = format!("{}.rs", path.display());
                if !fs::exists(&path_str).unwrap() {
                    path_str = format!("{}/mod.rs", path.display());
                }

                let entry = files_for_structs.entry(path_str).or_default();
                entry.push(struct_name.to_string());
            }
        }
    }

    let mut structs = Vec::new();
    let mut already_added = HashSet::new();
    loop {
        let mut new_structs = Vec::new();
        for (file, structs_to_find) in &files_for_structs {
            for (name, def) in StructDefinition::find(file, &src_tauri_path) {
                if structs_to_find.contains(&name) && already_added.insert(def.name.clone()) {
                    new_structs.push(def);
                }
            }
        }

        files_for_structs.clear();
        for struct_d in &new_structs {
            for pat in struct_d.get_inner_leafs() {
                let struct_name = pat.split("::").last().unwrap_or(&pat);

                if let Some(path) = pat.strip_prefix("crate::") {
                    let path = PathBuf::from(&src_tauri_path)
                        .join(path.replace("::", "/"))
                        .parent()
                        .unwrap()
                        .to_path_buf();
                    let mut path_str = format!("{}.rs", path.display());
                    if !fs::exists(&path_str).unwrap() {
                        path_str = format!("{}/mod.rs", path.display());
                    }

                    let entry = files_for_structs.entry(path_str).or_default();
                    entry.push(struct_name.to_string());
                }
            }
        }

        for ns in new_structs {
            structs.push(ns);
        }
        if files_for_structs.is_empty() {
            break;
        }
    }
    */
}
