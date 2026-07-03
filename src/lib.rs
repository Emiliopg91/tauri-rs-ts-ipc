pub mod commands;
pub mod commons;
pub mod enums;
pub mod events;
pub mod structs;

use std::{
    collections::HashSet,
    fs,
    path::{Path, PathBuf},
    process::exit,
    time::{Instant, SystemTime},
};

use syn::visit::Visit;

use crate::{
    commands::CommandDefinition,
    commons::{RsTsVisitor, TsType},
    enums::EnumDefinition,
    events::EventDefinition,
    structs::StructDefinition,
};

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
    println!("------------- Rust <-> Typescript Generator -------------");
    let t0 = Instant::now();
    println!("  Gathering source codes files...");
    let mut files = Vec::new();
    let latest = find_rs_files(src_tauri_path, &mut files).unwrap();
    println!("    Found {} files", files.len());
    let mut outdated = true;

    if let Ok(exists) = fs::exists(client_path)
        && exists
        && let Ok(metadata) = fs::metadata(client_path)
        && let Ok(modified) = metadata.modified()
        && modified >= latest
    {
        outdated = false;
    }

    if !outdated {
        println!("  Up to date, nothing to do")
    } else {
        let mut commands = Vec::new();
        let mut events = Vec::new();
        let mut used_structs = Vec::new();
        let mut used_enums = Vec::new();

        inspect_code(
            src_tauri_path,
            files,
            &mut commands,
            &mut events,
            &mut used_structs,
            &mut used_enums,
        );

        generate_files(
            project_dir,
            backend_dir,
            models_path,
            client_path,
            listener_path,
            &mut commands,
            &mut events,
            &mut used_structs,
            &mut used_enums,
        );
    }

    println!("  Finished after {:.3} seconds", t0.elapsed().as_secs_f64(),);
    println!("---------------------------------------------------------");
}

fn find_rs_files<T>(
    path: T,
    out: &mut Vec<(PathBuf, syn::File)>,
) -> Result<SystemTime, std::io::Error>
where
    T: AsRef<Path>,
{
    let mut latest = SystemTime::UNIX_EPOCH;
    for entry in fs::read_dir(path.as_ref())?.flatten() {
        let entry = entry.path();
        if entry.is_file() {
            if entry.extension().unwrap() == "rs" {
                let modified = fs::metadata(&entry)?.modified()?;
                if modified > latest {
                    latest = modified;
                }
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
            let dir_latest = find_rs_files(entry, out)?;
            if dir_latest > latest {
                latest = dir_latest;
            }
        }
    }

    Ok(latest)
}

fn generate_files(
    project_dir: &Path,
    backend_dir: &PathBuf,
    models_path: &PathBuf,
    client_path: &PathBuf,
    listener_path: &PathBuf,
    commands: &mut Vec<CommandDefinition>,
    events: &mut Vec<EventDefinition>,
    used_structs: &mut Vec<StructDefinition>,
    used_enums: &mut Vec<EnumDefinition>,
) {
    if !fs::exists(backend_dir).unwrap() {
        fs::create_dir_all(backend_dir).unwrap();
    }

    println!(
        "  Generating models file in '{}'",
        &models_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
    );
    let mut all_types: Vec<Box<dyn TsType>> = Vec::new();

    used_structs
        .into_iter()
        .for_each(|e| all_types.push(Box::new(e.clone())));
    used_enums
        .into_iter()
        .for_each(|e| all_types.push(Box::new(e.clone())));

    all_types.sort_by_key(|e| e.get_sort_key());

    commons::generate_file(models_path, &all_types);
    println!("    Done");

    println!(
        "  Generating client file in '{}'",
        &client_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
    );
    commands.sort_by_key(|e| e.name.clone());
    CommandDefinition::generate_file(client_path, commands);
    println!("    Done");

    println!(
        "  Generating listener file in '{}'",
        &listener_path
            .display()
            .to_string()
            .replace(&project_dir.display().to_string(), "")[1..]
    );
    events.sort_by_key(|e| e.name.clone());
    EventDefinition::generate_file(listener_path, events);
    println!("    Done");
}

fn inspect_code(
    src_tauri_path: &PathBuf,
    files: Vec<(PathBuf, syn::File)>,
    commands: &mut Vec<CommandDefinition>,
    events: &mut Vec<EventDefinition>,
    used_structs: &mut Vec<StructDefinition>,
    used_enums: &mut Vec<EnumDefinition>,
) {
    let mut structs = HashSet::new();
    let mut enums = HashSet::new();
    let mut inner_used_types = HashSet::new();

    println!("  Inspecting source code...");
    for file in &files {
        let mut finder = RsTsVisitor::new(file, src_tauri_path);
        finder.visit_file(&file.1);

        for cmd in &finder.commands {
            cmd.get_inner_leafs()
                .iter()
                .filter(|s| s.starts_with("crate::"))
                .for_each(|s| {
                    inner_used_types.insert(s.clone());
                });
            commands.push(cmd.clone());
        }

        for event in &finder.events {
            event
                .get_inner_leafs()
                .iter()
                .filter(|s| s.starts_with("crate::"))
                .for_each(|s| {
                    inner_used_types.insert(s.clone());
                });
            events.push(event.clone());
        }

        for enum_d in &finder.enums {
            enums.insert(enum_d.clone());
        }

        for struct_d in &finder.structs {
            struct_d
                .get_inner_leafs()
                .iter()
                .filter(|s| s.starts_with("crate::"))
                .for_each(|s| {
                    inner_used_types.insert(s.clone());
                });
            structs.insert(struct_d.clone());
        }
    }

    inner_used_types
        .iter()
        .filter_map(|f| {
            for struct_d in &structs {
                if struct_d.get_full_qualified_name() == *f {
                    return Some(struct_d.clone());
                }
            }
            None
        })
        .for_each(|e| used_structs.push(e));

    inner_used_types
        .iter()
        .filter_map(|f| {
            for enum_d in &enums {
                if enum_d.get_full_qualified_name() == *f {
                    return Some(enum_d.clone());
                }
            }
            None
        })
        .for_each(|e| used_enums.push(e));

    println!("    Found {} commands", commands.len());
    println!("    Found {} events", events.len());
    println!("    Found {} enums", used_enums.len());
    println!("    Found {} structs", used_structs.len());
}
