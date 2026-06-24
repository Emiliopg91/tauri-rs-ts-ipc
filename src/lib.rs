pub mod commands;
pub mod commons;
pub mod events;
pub mod structs;

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::exit,
};

use syn::visit::Visit;

use crate::{
    commands::CommandDefinition, commons::RsTsVisitor, events::EventDefinition,
    structs::StructDefinition,
};

fn find_rs_files<T>(path: T, out: &mut Vec<(PathBuf, syn::File)>) -> Result<(), std::io::Error>
where
    T: AsRef<Path>,
{
    for entry in fs::read_dir(path.as_ref())?.flatten() {
        let entry = entry.path();
        if entry.is_file() {
            if entry.extension().unwrap() == "rs" {
                match fs::read_to_string(&entry) {
                    Ok(content) => match syn::parse_file(&content) {
                        Ok(syn_file) => {
                            out.push((entry, syn_file));
                        }
                        Err(_) => exit(0),
                    },
                    Err(_) => exit(0),
                }
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
    project_dir: &Path,
    backend_dir: &PathBuf,
    src_tauri_path: &PathBuf,
    models_path: &PathBuf,
    client_path: &PathBuf,
    listener_path: &PathBuf,
) {
    println!("cargo:warning=Gathering Rust source codes...");
    let mut files = Vec::new();
    find_rs_files(src_tauri_path, &mut files).unwrap();

    let mut commands = Vec::new();
    let mut events = Vec::new();
    let mut structs = HashSet::new();
    let mut used_structs = HashSet::new();

    println!("cargo:warning=Inspecting source code...");
    for file in &files {
        let mut finder = RsTsVisitor::new(file, src_tauri_path);
        finder.visit_file(&file.1);

        for cmd in &finder.commands {
            cmd.get_inner_leafs()
                .iter()
                .filter(|s| s.starts_with("crate::"))
                .for_each(|s| {
                    used_structs.insert(s.clone());
                });
            commands.push(cmd.clone());
        }

        for event in &finder.events {
            event
                .get_inner_leafs()
                .iter()
                .filter(|s| s.starts_with("crate::"))
                .for_each(|s| {
                    used_structs.insert(s.clone());
                });
            events.push(event.clone());
        }

        for struct_d in &finder.structs {
            struct_d
                .get_inner_leafs()
                .iter()
                .filter(|s| s.starts_with("crate::"))
                .for_each(|s| {
                    used_structs.insert(s.clone());
                });
            structs.insert(struct_d.clone());
        }
    }

    let mut used_structs = used_structs
        .iter()
        .filter_map(|f| {
            for struct_d in &structs {
                if struct_d.get_full_qualified_name() == *f {
                    return Some(struct_d.clone());
                }
            }
            None
        })
        .collect::<Vec<StructDefinition>>();

    println!("cargo:warning=  Found {} commands", commands.len());
    println!("cargo:warning=  Found {} events", events.len());
    println!("cargo:warning=  Found {} structs", used_structs.len());

    if !fs::exists(backend_dir).unwrap() {
        fs::create_dir_all(backend_dir).unwrap();
    }

    println!(
        "cargo:warning=Generating models file in '{}'",
        &models_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
    );
    used_structs.sort_by_key(|e| e.name.clone());
    StructDefinition::generate_file(models_path, used_structs);
    println!("cargo:warning=  Done");

    println!(
        "cargo:warning=Generating client file in '{}'",
        &client_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
    );
    commands.sort_by_key(|e| e.name.clone());
    CommandDefinition::generate_file(client_path, commands);
    println!("cargo:warning=  Done");

    println!(
        "cargo:warning=Generating listener file in '{}'",
        &listener_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
    );
    events.sort_by_key(|e| e.name.clone());
    EventDefinition::generate_file(listener_path, events);
    println!("cargo:warning=  Done");
}
