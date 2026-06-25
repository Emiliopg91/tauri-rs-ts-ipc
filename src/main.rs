use std::path::PathBuf;

use tauri_rs_ts_ipc::inner_build;

fn main() {
    let src_tauri_path_buf =
        PathBuf::from("/var/mnt/Datos/Desarrollo/Workspace/VSCode/taurfit/src-tauri");
    let src_tauri_path = src_tauri_path_buf.as_path().join("src");
    let project_dir = src_tauri_path_buf.as_path().parent().unwrap().to_path_buf();
    let backend_dir = PathBuf::from("/var/mnt/Datos/Desarrollo/Workspace/VSCode/syn-test/backend");
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
