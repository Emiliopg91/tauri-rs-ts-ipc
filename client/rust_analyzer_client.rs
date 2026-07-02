//! Cliente mínimo de `rust-analyzer` por LSP (stdio).
//!
//! Dependencias (Cargo.toml):
//! ```toml
//! [dependencies]
//! serde_json = "1"
//! ```
//!
//! Uso:
//! ```no_run
//! use rust_analyzer_client::RustAnalyzerClient;
//!
//! fn main() -> std::io::Result<()> {
//!     let mut client = RustAnalyzerClient::start("/ruta/al/proyecto")?;
//!     // línea y columna en base 1 (como en un editor)
//!     if let Some(info) = client.type_at("/ruta/al/proyecto/src/main.rs", 10, 5)? {
//!         println!("Tipo: {:?}", info.type_string);
//!         println!("Hover crudo:\n{}", info.raw_hover);
//!     }
//!     client.shutdown()?;
//!     Ok(())
//! }
//! ```

use serde_json::{json, Value};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Error, ErrorKind, Read, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// Resultado de consultar el tipo en una posición concreta.
#[derive(Debug, Clone)]
pub struct TypeInfo {
    /// Contenido crudo devuelto por el hover de rust-analyzer (markdown).
    pub raw_hover: String,
    /// Mejor intento de extraer solo el tipo (heurístico, puede fallar en casos complejos).
    pub type_string: Option<String>,
}

/// Cliente que mantiene un proceso `rust-analyzer` vivo y habla LSP con él.
pub struct RustAnalyzerClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
    opened_files: HashSet<String>,
}

impl RustAnalyzerClient {
    /// Arranca `rust-analyzer` (debe estar en el PATH) para el proyecto en `project_root`
    /// y realiza el handshake `initialize` / `initialized`.
    pub fn start(project_root: impl AsRef<Path>) -> std::io::Result<Self> {
        let root = project_root.as_ref().canonicalize()?;
        let root_uri = path_to_uri(&root);

        let mut child = Command::new("rust-analyzer")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                Error::new(
                    e.kind(),
                    format!("no se pudo lanzar `rust-analyzer` (¿está en el PATH?): {e}"),
                )
            })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            Error::new(ErrorKind::BrokenPipe, "no se pudo abrir stdin de rust-analyzer")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            Error::new(ErrorKind::BrokenPipe, "no se pudo abrir stdout de rust-analyzer")
        })?;

        let mut client = RustAnalyzerClient {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            next_id: 1,
            opened_files: HashSet::new(),
        };

        let init_params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "hover": {
                        "contentFormat": ["markdown", "plaintext"]
                    }
                }
            }
        });
        client.send_request("initialize", init_params)?;
        client.send_notification("initialized", json!({}))?;

        Ok(client)
    }

    /// Devuelve el tipo de la expresión que empieza en `line`/`column` (base 1, como en un
    /// editor) dentro de `file_path`. `None` si rust-analyzer no devuelve información de hover
    /// para esa posición.
    pub fn type_at(
        &mut self,
        file_path: impl AsRef<Path>,
        line: u32,
        column: u32,
    ) -> std::io::Result<Option<TypeInfo>> {
        let path = file_path.as_ref().canonicalize()?;
        let uri = path_to_uri(&path);
        self.ensure_open(&path, &uri)?;

        // LSP usa posiciones base 0.
        let params = json!({
            "textDocument": { "uri": uri },
            "position": {
                "line": line.saturating_sub(1),
                "character": column.saturating_sub(1)
            }
        });

        let result = self.send_request("textDocument/hover", params)?;
        if result.is_null() {
            return Ok(None);
        }

        let raw_hover = extract_hover_text(&result);
        if raw_hover.trim().is_empty() {
            return Ok(None);
        }

        let type_string = extract_type_from_hover(&raw_hover);
        Ok(Some(TypeInfo { raw_hover, type_string }))
    }

    /// Cierra la sesión LSP de forma ordenada (`shutdown` + `exit`).
    pub fn shutdown(&mut self) -> std::io::Result<()> {
        let _ = self.send_request("shutdown", Value::Null);
        self.send_notification("exit", Value::Null)?;
        let _ = self.child.wait();
        Ok(())
    }

    // --- privados -------------------------------------------------------

    fn ensure_open(&mut self, path: &Path, uri: &str) -> std::io::Result<()> {
        if self.opened_files.contains(uri) {
            return Ok(());
        }
        let text = std::fs::read_to_string(path)?;
        let params = json!({
            "textDocument": {
                "uri": uri,
                "languageId": "rust",
                "version": 1,
                "text": text
            }
        });
        self.send_notification("textDocument/didOpen", params)?;
        self.opened_files.insert(uri.to_string());
        Ok(())
    }

    fn send_request(&mut self, method: &str, params: Value) -> std::io::Result<Value> {
        let id = self.next_id;
        self.next_id += 1;

        let msg = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        self.write_message(&msg)?;

        // Puede haber notificaciones intermedias (p.ej. publishDiagnostics);
        // las descartamos hasta encontrar la respuesta con el id esperado.
        loop {
            let msg = self.read_message()?;
            if msg.get("id") == Some(&Value::from(id)) {
                if let Some(err) = msg.get("error") {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("rust-analyzer devolvió error: {err}"),
                    ));
                }
                return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
            }
            // Notificación u otro mensaje: se ignora y se sigue leyendo.
        }
    }

    fn send_notification(&mut self, method: &str, params: Value) -> std::io::Result<()> {
        let msg = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });
        self.write_message(&msg)
    }

    fn write_message(&mut self, value: &Value) -> std::io::Result<()> {
        let body = serde_json::to_vec(value)?;
        write!(self.stdin, "Content-Length: {}\r\n\r\n", body.len())?;
        self.stdin.write_all(&body)?;
        self.stdin.flush()
    }

    fn read_message(&mut self) -> std::io::Result<Value> {
        let mut content_length: Option<usize> = None;
        loop {
            let mut line = String::new();
            if self.stdout.read_line(&mut line)? == 0 {
                return Err(Error::new(
                    ErrorKind::UnexpectedEof,
                    "rust-analyzer cerró stdout inesperadamente",
                ));
            }
            let line = line.trim_end();
            if line.is_empty() {
                break; // fin de cabeceras
            }
            if let Some(rest) = line.strip_prefix("Content-Length:") {
                content_length = Some(rest.trim().parse().map_err(|e| {
                    Error::new(ErrorKind::InvalidData, format!("Content-Length inválido: {e}"))
                })?);
            }
        }

        let len = content_length.ok_or_else(|| {
            Error::new(ErrorKind::InvalidData, "mensaje LSP sin cabecera Content-Length")
        })?;

        let mut buf = vec![0u8; len];
        self.stdout.read_exact(&mut buf)?;
        serde_json::from_slice(&buf)
            .map_err(|e| Error::new(ErrorKind::InvalidData, format!("JSON inválido: {e}")))
    }
}

impl Drop for RustAnalyzerClient {
    fn drop(&mut self) {
        let _ = self.shutdown();
        let _ = self.child.kill();
    }
}

fn path_to_uri(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        // Rutas tipo Windows "C:/..."
        format!("file:///{p}")
    }
}

/// Extrae el texto (markdown/plaintext) del resultado de `textDocument/hover`.
/// El campo `contents` puede venir como `MarkupContent`, `MarkedString` o `MarkedString[]`.
fn extract_hover_text(result: &Value) -> String {
    let Some(contents) = result.get("contents") else {
        return String::new();
    };

    match contents {
        Value::Object(obj) => {
            if let Some(Value::String(s)) = obj.get("value") {
                return s.clone();
            }
            String::new()
        }
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => s.clone(),
                Value::Object(o) => o
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string(),
                _ => String::new(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Heurística para sacar solo el tipo del texto de hover de rust-analyzer.
///
/// Cubre los formatos más comunes:
/// - `i32` (hover directo sobre una expresión)
/// - `let x: i32` / `let mut x: i32`
/// - `const X: i32 = 1`
/// - `fn foo(...) -> i32`
///
/// No es un parser real de Rust: para tipos genéricos o firmas muy complejas puede
/// devolver `None` y conviene mirar `raw_hover` a mano.
fn extract_type_from_hover(raw: &str) -> Option<String> {
    let code = first_fenced_code_block(raw)?;
    let first_line = code.lines().next()?.trim();

    if let Some(idx) = first_line.find("->") {
        return Some(first_line[idx + 2..].trim().trim_end_matches('{').trim().to_string());
    }

    if first_line.starts_with("let ") || first_line.starts_with("const ") || first_line.starts_with("static ") {
        if let Some(colon) = first_line.find(':') {
            let after_colon = &first_line[colon + 1..];
            let end = after_colon.find('=').unwrap_or(after_colon.len());
            let ty = after_colon[..end].trim();
            if !ty.is_empty() {
                return Some(ty.to_string());
            }
        }
        return None;
    }

    // Hover "desnudo" sobre una expresión: rust-analyzer suele devolver solo el tipo.
    if !first_line.is_empty() && !first_line.contains(' ') {
        return Some(first_line.to_string());
    }

    None
}

fn first_fenced_code_block(raw: &str) -> Option<String> {
    let mut lines = raw.lines();
    while let Some(line) = lines.next() {
        if line.trim_start().starts_with("```") {
            let mut block = String::new();
            for l in lines.by_ref() {
                if l.trim_start().starts_with("```") {
                    return Some(block);
                }
                if !block.is_empty() {
                    block.push('\n');
                }
                block.push_str(l);
            }
        }
    }
    None
}
