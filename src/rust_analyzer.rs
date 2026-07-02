//! Cliente mínimo de `rust-analyzer` por LSP (stdio), con warmup de indexación.
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
//! use std::time::Duration;
//!
//! fn main() -> std::io::Result<()> {
//!     let mut client = RustAnalyzerClient::start("/ruta/al/proyecto")?;
//!
//!     // Espera (hasta 60s) a que rust-analyzer termine de indexar el proyecto.
//!     // Devuelve true si confirmó el fin de la indexación, false si hizo timeout
//!     // (en ese caso puedes seguir igualmente, solo que el primer hover puede
//!     // tardar más o ser menos preciso).
//!     let ready = client.warmup(Duration::from_secs(60))?;
//!     eprintln!("indexación completa: {ready}");
//!
//!     if let Some(info) = client.type_at("/ruta/al/proyecto/src/main.rs", 10, 5)? {
//!         println!("Tipo: {:?}", info.type_string);
//!         println!("Hover crudo:\n{}", info.raw_hover);
//!     }
//!     client.shutdown()?;
//!     Ok(())
//! }
//! ```

use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Error, ErrorKind, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// Resultado de consultar el tipo en una posición concreta.
#[derive(Debug, Clone, Serialize)]
pub struct TypeInfo {
    /// Contenido crudo devuelto por el hover de rust-analyzer (markdown).
    pub raw_hover: String,
    /// Mejor intento de extraer solo el tipo (heurístico, puede fallar en casos complejos).
    pub type_string: Option<String>,
}

/// Cliente que mantiene un proceso `rust-analyzer` vivo y habla LSP con él.
///
/// La lectura del stdout del servidor corre en un hilo aparte, que:
/// - reenvía notificaciones (incluidas `$/progress`, usadas para el warmup) y
///   respuestas a nuestras peticiones a través de un canal.
/// - responde automáticamente (con `null`) a peticiones que el servidor le
///   hace al cliente (p.ej. `window/workDoneProgress/create`), para no
///   dejarlas colgadas.
pub struct RustAnalyzerClient {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    rx: Receiver<Value>,
    next_id: i64,
    opened_files: HashSet<String>,
    /// Tokens de progreso (`$/progress`) actualmente activos (begin/report sin end).
    active_progress: HashSet<String>,
    /// Si hemos visto alguna notificación de progreso desde que arrancamos.
    seen_any_progress: bool,
    _reader_thread: JoinHandle<()>,
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
            .map_err(|e| Error::new(e.kind(), format!("rust-analyzer not found in path: {e}")))?;

        let stdin = child.stdin.take().ok_or_else(|| {
            Error::new(ErrorKind::BrokenPipe, "Could not get rust-analyzer stdin")
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            Error::new(ErrorKind::BrokenPipe, "Could not get rust-analyzer stdout")
        })?;

        let stdin = Arc::new(Mutex::new(stdin));
        let (tx, rx) = mpsc::channel::<Value>();

        let stdin_for_thread = Arc::clone(&stdin);
        let reader_thread = thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            loop {
                let msg = match read_framed(&mut reader) {
                    Ok(m) => m,
                    Err(_) => break, // stdout cerrado o mensaje corrupto: paramos el hilo
                };

                let method = msg
                    .get("method")
                    .and_then(|m| m.as_str())
                    .map(str::to_string);
                let has_id = msg.get("id").is_some();

                match (method, has_id) {
                    (Some(method), true) => {
                        // Petición del servidor hacia el cliente: la contestamos
                        // automáticamente para no dejar al servidor esperando.
                        let id = msg.get("id").cloned().unwrap_or(Value::Null);
                        let result = default_response_for(&method, msg.get("params"));
                        let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
                        if let Ok(mut guard) = stdin_for_thread.lock() {
                            let _ = write_framed(&mut *guard, &response);
                        }
                    }
                    _ => {
                        // Notificación (incluye $/progress) o respuesta a algo que
                        // nosotros pedimos: se reenvía para que el hilo principal la procese.
                        if tx.send(msg).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let mut client = RustAnalyzerClient {
            child,
            stdin,
            rx,
            next_id: 1,
            opened_files: HashSet::new(),
            active_progress: HashSet::new(),
            seen_any_progress: false,
            _reader_thread: reader_thread,
        };

        let init_params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "capabilities": {
                "textDocument": {
                    "hover": {
                        "contentFormat": ["markdown", "plaintext"]
                    }
                },
                "window": {
                    "workDoneProgress": true
                }
            }
        });
        client.send_request("initialize", init_params)?;
        client.send_notification("initialized", json!({}))?;

        Ok(client)
    }

    /// Espera a que rust-analyzer termine de indexar el proyecto, escuchando las
    /// notificaciones `$/progress` que emite durante el arranque (indexado de
    /// crates, cache priming, etc.).
    ///
    /// Devuelve `Ok(true)` si se confirmó el fin de la indexación, u `Ok(false)`
    /// si se agotó `timeout` sin confirmación (puedes seguir usando el cliente
    /// igualmente; simplemente el primer hover podría ser más lento o menos
    /// completo si aún se está indexando).
    pub fn warmup(&mut self, timeout: Duration) -> std::io::Result<bool> {
        let deadline = Instant::now() + timeout;
        // Si en este margen no ha aparecido ningún progreso, asumimos que el
        // proyecto no necesitaba indexación (p.ej. un solo archivo suelto).
        let grace_deadline = Instant::now() + timeout.min(Duration::from_secs(2));

        loop {
            if self.seen_any_progress && self.active_progress.is_empty() {
                return Ok(true);
            }
            if !self.seen_any_progress && Instant::now() >= grace_deadline {
                return Ok(true);
            }

            let now = Instant::now();
            if now >= deadline {
                return Ok(false);
            }
            let remaining = deadline - now;

            match self.recv_next(Some(remaining))? {
                Some(_) => continue,      // el bookkeeping de progreso ya se aplicó dentro
                None => return Ok(false), // timeout
            }
        }
    }

    /// Devuelve el tipo de la expresión que empieza en `line`/`column` (base 1, como en un
    /// editor) dentro de `file_path`. `None` si rust-analyzer no devuelve información de hover
    /// para esa posición.
    pub fn type_at(
        &mut self,
        file_path: impl AsRef<Path>,
        line: usize,
        column: usize,
    ) -> std::io::Result<Option<TypeInfo>> {
        let path = file_path.as_ref().canonicalize()?;
        let uri = path_to_uri(&path);
        self.ensure_open(&path, &uri)?;

        // LSP usa posiciones base 0.
        let params = json!({
            "textDocument": { "uri": uri },
            "position": {
                "line": line.saturating_sub(1),
                "character": column
            }
        });

        let mut raw_hover = "".to_string();
        let mut type_string = None;

        let mut retries = 300;
        loop {
            let result = self.send_request("textDocument/hover", params.clone())?;

            if !result.is_null() {
                raw_hover = extract_hover_text(&result);
                if raw_hover.trim().is_empty() {
                    return Ok(None);
                }

                type_string = extract_type_from_hover(&raw_hover);
            }

            if type_string.is_some() {
                return Ok(Some(TypeInfo {
                    raw_hover,
                    type_string,
                }));
            }

            if retries == 0 {
                break;
            }

            retries -= 1;
            std::thread::sleep(Duration::from_millis(100));
        }
        return Ok(None);
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
        self.seen_any_progress = false;
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

        loop {
            // timeout None: bloquea hasta recibir algo (o error de conexión).
            let incoming = self.recv_next(None)?.unwrap();

            if incoming.get("id") == Some(&Value::from(id)) {
                if let Some(err) = incoming.get("error") {
                    return Err(Error::new(
                        ErrorKind::Other,
                        format!("rust-analyzer exited with error: {err}"),
                    ));
                }
                return Ok(incoming.get("result").cloned().unwrap_or(Value::Null));
            }
            // Notificación (p.ej. $/progress, publishDiagnostics): ya se ha
            // hecho el bookkeeping en recv_next; seguimos esperando la respuesta.
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

    fn write_message(&self, value: &Value) -> std::io::Result<()> {
        let mut guard = self
            .stdin
            .lock()
            .map_err(|_| Error::new(ErrorKind::Other, "Poisoned stdin"))?;
        write_framed(&mut *guard, value)
    }

    /// Recibe el siguiente mensaje del hilo lector (notificación o respuesta),
    /// actualizando el estado de progreso usado por `warmup`.
    fn recv_next(&mut self, timeout: Option<Duration>) -> std::io::Result<Option<Value>> {
        let msg = match timeout {
            Some(t) => match self.rx.recv_timeout(t) {
                Ok(m) => m,
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(Error::new(ErrorKind::BrokenPipe, "Lost connection"));
                }
            },
            None => self
                .rx
                .recv()
                .map_err(|_| Error::new(ErrorKind::BrokenPipe, "Lost connection"))?,
        };
        self.bookkeep_progress(&msg);
        Ok(Some(msg))
    }

    fn bookkeep_progress(&mut self, msg: &Value) {
        if msg.get("method").and_then(|m| m.as_str()) != Some("$/progress") {
            return;
        }
        let Some(params) = msg.get("params") else {
            return;
        };
        let token = params
            .get("token")
            .map(|t| t.to_string())
            .unwrap_or_default();
        let Some(value) = params.get("value") else {
            return;
        };
        let kind = value.get("kind").and_then(|k| k.as_str()).unwrap_or("");

        match kind {
            "begin" | "report" => {
                self.active_progress.insert(token);
                self.seen_any_progress = true;
            }
            "end" => {
                self.active_progress.remove(&token);
            }
            _ => {}
        }
    }
}

impl Drop for RustAnalyzerClient {
    fn drop(&mut self) {
        let _ = self.shutdown();
        let _ = self.child.kill();
    }
}

fn write_framed<W: Write>(w: &mut W, value: &Value) -> std::io::Result<()> {
    let body = serde_json::to_vec(value)?;
    write!(w, "Content-Length: {}\r\n\r\n", body.len())?;
    w.write_all(&body)?;
    w.flush()
}

fn read_framed<R: BufRead>(r: &mut R) -> std::io::Result<Value> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        if r.read_line(&mut line)? == 0 {
            return Err(Error::new(ErrorKind::UnexpectedEof, "Closed connection"));
        }
        let line = line.trim_end();
        if line.is_empty() {
            break; // fin de cabeceras
        }
        if let Some(rest) = line.strip_prefix("Content-Length:") {
            content_length = Some(rest.trim().parse().map_err(|e| {
                Error::new(
                    ErrorKind::InvalidData,
                    format!("Invalid Content-Length: {e}"),
                )
            })?);
        }
    }

    let len = content_length.ok_or_else(|| {
        Error::new(
            ErrorKind::InvalidData,
            "Message withou Content-Length header",
        )
    })?;

    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    serde_json::from_slice(&buf)
        .map_err(|e| Error::new(ErrorKind::InvalidData, format!("JSON inválido: {e}")))
}

/// Respuesta por defecto para peticiones que el servidor hace al cliente y que
/// este cliente mínimo no necesita atender de forma especial.
fn default_response_for(method: &str, params: Option<&Value>) -> Value {
    match method {
        // rust-analyzer puede pedir configuración por cada item declarado; hay
        // que devolver un array de la misma longitud (con null es suficiente
        // para que use sus valores por defecto).
        "workspace/configuration" => {
            let n = params
                .and_then(|p| p.get("items"))
                .and_then(|i| i.as_array())
                .map(|a| a.len())
                .unwrap_or(1);
            Value::Array(vec![Value::Null; n])
        }
        _ => Value::Null,
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
        return Some(
            first_line[idx + 2..]
                .trim()
                .trim_end_matches('{')
                .trim()
                .to_string(),
        );
    }

    if first_line.starts_with("let ")
        || first_line.starts_with("const ")
        || first_line.starts_with("static ")
    {
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
