use std::{
    io::{BufRead, BufReader, Read},
    path::PathBuf,
};

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

    use std::io::Write;
    use std::process::{Command, Stdio};

    let mut child = Command::new("rust-analyzer")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();

    let msg = r#"{
    "jsonrpc":"2.0",
    "id":1,
    "method":"initialize",
    "params":{
        "rootUri":"file:///var/mnt/Datos/Desarrollo/Workspace/VSCode/taurfit/src-tauri",
    "capabilities": {}
    }
}"#;

    let payload = format!("Content-Length: {}\r\n\r\n{}", msg.len(), msg);

    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();

    let stdout = child.stdout.take().unwrap();
    let mut reader = BufReader::new(stdout);

    loop {
        let mut content_length = None;

        // Leer cabeceras
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();

            let line = line.trim_end();

            if line.is_empty() {
                break; // fin de cabeceras
            }

            if let Some(v) = line.strip_prefix("Content-Length: ") {
                content_length = Some(v.parse::<usize>().unwrap());
            }
        }

        let len = content_length.unwrap();

        // Leer cuerpo JSON
        let mut body = vec![0; len];
        reader.read_exact(&mut body).unwrap();

        let json = String::from_utf8(body).unwrap();

        println!("Mensaje recibido:");
        println!("{json}");
    }
}
