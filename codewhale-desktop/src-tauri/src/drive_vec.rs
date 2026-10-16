//! Semantic search over `git_vault` for the ai-drive MCP server.
//!
//! `mcp-drive embed` walks git_vault the same way the FTS indexer does, cuts every source or
//! documentation file into overlapping windows of lines, asks Ollama (`POST /api/embed`) for
//! an embedding of each window and stores the vectors in `<root>/vectordb/vectors.sqlite`.
//! `semantic_search` embeds the question the same way, scores every chunk by cosine
//! similarity and, in hybrid mode, fuses that ranking with the FTS5 hits of `code_search`
//! (reciprocal rank fusion), so a question in plain words finds the right file even when
//! none of its identifiers appear in the query.
//!
//! Vectors are unit-normalised and stored as int8 with a per-vector scale (768 bytes per
//! chunk for nomic-embed-text), which keeps a 100k-chunk index under 100 MB and lets the
//! long-lived MCP process keep the whole matrix in memory. Rebuilds are incremental: a file
//! whose content hash is unchanged keeps its vectors, so re-indexing after `git pull` only
//! embeds what changed.

use crate::drive_mcp::{self, DriveEnv};
use crate::mcp_core::now_iso;
use rusqlite::{Connection, OpenFlags, params};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

pub const VECTORS_REL: &str = "vectordb/vectors.sqlite";
pub const DEFAULT_EMBED_MODEL: &str = "nomic-embed-text";
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";
/// Window and stride in lines: 40-line chunks that overlap by 8 lines.
pub const WINDOW_LINES: usize = 40;
pub const STRIDE_LINES: usize = 32;
const MAX_CHUNK_CHARS: usize = 3000;
const MIN_CHUNK_CHARS: usize = 40;
const MAX_LINE_CHARS: usize = 400;
const MAX_EMBED_FILE: u64 = 256 * 1024;
/// Chunks per `/api/embed` request.
const BATCH: usize = 32;
const PREVIEW_CHARS: usize = 160;
const RRF_K: f32 = 60.0;
/// Source and documentation formats worth embedding (scenes, resources and data files are not).
const EMBED_EXT: &[&str] = &[
    "gd", "gdshader", "gdshaderinc", "glsl", "shader", "hlsl", "vert", "frag", "comp", "cpp", "cc",
    "cxx", "h", "hpp", "hh", "c", "cs", "py", "rs", "js", "ts", "tsx", "jsx", "java", "kt", "swift",
    "go", "lua", "md", "rst", "txt", "cfg", "toml", "yaml", "yml", "ini",
];

// ---------------------------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct VectorProgress {
    pub files: u64,
    pub chunks: u64,
    pub embedded: u64,
    pub reused: u64,
    pub skipped: u64,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VectorSummary {
    pub path: String,
    pub model: String,
    pub dim: usize,
    pub files: u64,
    pub chunks: u64,
    pub embedded: u64,
    pub reused: u64,
    pub skipped: u64,
    pub elapsed_ms: u64,
}

#[derive(Serialize, Debug, Clone)]
#[serde(rename_all = "camelCase")]
pub struct VectorIndexInfo {
    pub path: String,
    pub bytes: u64,
    pub model: Option<String>,
    pub dim: Option<usize>,
    pub files: Option<u64>,
    pub chunks: Option<u64>,
    pub built_at: Option<String>,
}

#[derive(Serialize, Debug, Clone, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SemanticHit {
    pub path: String,
    pub repo: String,
    /// Best-matching window (0 when the file came only from the FTS ranking).
    pub start_line: usize,
    pub end_line: usize,
    /// Cosine similarity of the best window, when the file matched by meaning.
    pub similarity: Option<f32>,
    /// "vector", "fts" or "both".
    pub source: String,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chunk {
    pub start_line: usize,
    pub end_line: usize,
    pub text: String,
}

/// Anything that turns text into vectors; Ollama in production, a bag-of-words fake in tests.
pub trait Embedder {
    fn model(&self) -> &str;
    /// One vector per input, same order. `query` marks search queries (some models want a
    /// different prefix for queries and documents).
    fn embed(&self, inputs: &[String], query: bool) -> Result<Vec<Vec<f32>>, String>;
}

// ---------------------------------------------------------------------------------------------
// Ollama embeddings
// ---------------------------------------------------------------------------------------------

pub struct OllamaEmbedder {
    url: String,
    model: String,
    client: reqwest::blocking::Client,
}

impl OllamaEmbedder {
    pub fn new(url: &str, model: &str) -> Result<Self, String> {
        let client = reqwest::blocking::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(600))
            .build()
            .map_err(|e| e.to_string())?;
        Ok(Self {
            url: url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            client,
        })
    }

    /// nomic-embed-text is trained with task prefixes; other models take the raw text.
    fn prefixed(&self, text: &str, query: bool) -> String {
        if self.model.to_ascii_lowercase().contains("nomic") {
            let prefix = if query { "search_query: " } else { "search_document: " };
            format!("{prefix}{text}")
        } else {
            text.to_string()
        }
    }
}

impl Embedder for OllamaEmbedder {
    fn model(&self) -> &str {
        &self.model
    }

    fn embed(&self, inputs: &[String], query: bool) -> Result<Vec<Vec<f32>>, String> {
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        let body = json!({
            "model": self.model,
            "input": inputs.iter().map(|s| self.prefixed(s, query)).collect::<Vec<_>>(),
            "truncate": true,
            "keep_alive": "10m"
        });
        let resp = self
            .client
            .post(format!("{}/api/embed", self.url))
            .json(&body)
            .send()
            .map_err(|e| format!("Ollama недоступна ({}): {e}", self.url))?;
        let status = resp.status();
        let v: Value = resp
            .json()
            .map_err(|e| format!("Ollama вернула не JSON: {e}"))?;
        if !status.is_success() {
            let msg = v["error"].as_str().unwrap_or("").to_string();
            let hint = if msg.contains("not found") {
                format!(" Скачайте модель: ollama pull {}", self.model)
            } else {
                String::new()
            };
            return Err(format!("Ollama ответила HTTP {}: {msg}.{hint}", status.as_u16()));
        }
        let arr = v["embeddings"]
            .as_array()
            .ok_or("Ollama не вернула embeddings (нужна Ollama 0.3+ и модель эмбеддингов)")?;
        if arr.len() != inputs.len() {
            return Err(format!(
                "Ollama вернула {} векторов на {} запросов",
                arr.len(),
                inputs.len()
            ));
        }
        arr.iter()
            .map(|e| {
                e.as_array()
                    .map(|a| a.iter().filter_map(Value::as_f64).map(|f| f as f32).collect())
            })
            .collect::<Option<Vec<Vec<f32>>>>()
            .ok_or_else(|| "Ollama вернула вектор не из чисел".to_string())
    }
}

// ---------------------------------------------------------------------------------------------
// Chunking and vector maths
// ---------------------------------------------------------------------------------------------

fn clip_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// Cuts `text` into windows of `window` lines every `stride` lines. Windows shorter than
/// `MIN_CHUNK_CHARS` (blank stretches) are dropped; very long lines are clipped.
pub fn chunk_lines(text: &str, window: usize, stride: usize) -> Vec<Chunk> {
    let window = window.max(1);
    let stride = stride.clamp(1, window);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    if lines.is_empty() {
        return out;
    }
    let mut start = 0;
    loop {
        let end = (start + window).min(lines.len());
        let mut body = String::new();
        for line in &lines[start..end] {
            body.push_str(clip_chars(line, MAX_LINE_CHARS));
            body.push('\n');
            if body.len() > MAX_CHUNK_CHARS {
                break;
            }
        }
        let trimmed = body.trim();
        if trimmed.chars().count() >= MIN_CHUNK_CHARS {
            out.push(Chunk {
                start_line: start + 1,
                end_line: end,
                text: trimmed.to_string(),
            });
        }
        if end >= lines.len() {
            break;
        }
        start += stride;
    }
    out
}

pub fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// int8 quantisation with a per-vector scale: `x ≈ q * scale / 127`.
pub fn quantize(v: &[f32]) -> (f32, Vec<i8>) {
    let scale = v.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if scale == 0.0 {
        return (0.0, vec![0; v.len()]);
    }
    let q = v
        .iter()
        .map(|x| (x / scale * 127.0).round().clamp(-127.0, 127.0) as i8)
        .collect();
    (scale, q)
}

/// Cosine similarity of two quantised unit vectors.
pub fn cosine_q8(a_scale: f32, a: &[i8], b_scale: f32, b: &[i8]) -> f32 {
    let dot: i32 = a
        .iter()
        .zip(b)
        .map(|(x, y)| i32::from(*x) * i32::from(*y))
        .sum();
    dot as f32 * a_scale * b_scale / (127.0 * 127.0)
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn embeddable(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| EMBED_EXT.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

fn preview_of(text: &str) -> String {
    let one_line: String = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    clip_chars(&one_line, PREVIEW_CHARS).to_string()
}

fn repo_of(rel: &str) -> String {
    rel.split('/').take(2).collect::<Vec<_>>().join("/")
}

fn repo_matches(repo: &str, filter: Option<&str>) -> bool {
    match filter {
        None => true,
        Some(f) => {
            let f = f.trim_matches('/');
            repo == f || repo.starts_with(&format!("{f}/"))
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Walking git_vault
// ---------------------------------------------------------------------------------------------

struct VaultFile {
    rel: String,
    repo: String,
    text: String,
    hash: i64,
}

/// Text files under git_vault worth embedding, in a stable order.
fn walk_vault(vault: &Path, repo_filter: Option<&str>, skipped: &mut u64) -> Vec<VaultFile> {
    let mut out = Vec::new();
    let mut stack = vec![vault.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                if !drive_mcp::SKIP_DIRS.contains(&name.as_str()) {
                    stack.push(path);
                }
                continue;
            }
            let rel = path
                .strip_prefix(vault)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            let repo = repo_of(&rel);
            if !repo_matches(&repo, repo_filter) {
                continue;
            }
            if !embeddable(&path) || fs::metadata(&path).map(|m| m.len()).unwrap_or(0) > MAX_EMBED_FILE {
                *skipped += 1;
                continue;
            }
            let Some(text) = drive_mcp::indexable_text(&path) else {
                *skipped += 1;
                continue;
            };
            let hash = fnv1a64(text.as_bytes()) as i64;
            out.push(VaultFile { rel, repo, text, hash });
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Building the index
// ---------------------------------------------------------------------------------------------

fn open_rw(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("Не удалось открыть {}: {e}", path.display()))?;
    conn.execute_batch(
        "PRAGMA journal_mode = WAL; PRAGMA synchronous = NORMAL; PRAGMA temp_store = MEMORY;
         CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT);
         CREATE TABLE IF NOT EXISTS files(id INTEGER PRIMARY KEY, path TEXT UNIQUE NOT NULL, repo TEXT NOT NULL, hash INTEGER NOT NULL, chunks INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS chunks(id INTEGER PRIMARY KEY, file_id INTEGER NOT NULL, start_line INTEGER NOT NULL, end_line INTEGER NOT NULL, preview TEXT NOT NULL, scale REAL NOT NULL, vec BLOB NOT NULL);
         CREATE INDEX IF NOT EXISTS chunks_file ON chunks(file_id);",
    )
    .map_err(|e| e.to_string())?;
    Ok(conn)
}

fn meta_get(conn: &Connection, key: &str) -> Option<String> {
    conn.query_row("SELECT value FROM meta WHERE key = ?1", params![key], |r| r.get(0))
        .ok()
}

fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<(), String> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )
    .map(|_| ())
    .map_err(|e| e.to_string())
}

struct Pending {
    file_id: i64,
    chunk: Chunk,
}

fn flush_batch(
    conn: &Connection,
    embedder: &dyn Embedder,
    pending: &mut Vec<Pending>,
    dim: &mut Option<usize>,
) -> Result<u64, String> {
    if pending.is_empty() {
        return Ok(0);
    }
    let texts: Vec<String> = pending.iter().map(|p| p.chunk.text.clone()).collect();
    let vectors = embedder.embed(&texts, false)?;
    let mut stmt = conn
        .prepare("INSERT INTO chunks(file_id, start_line, end_line, preview, scale, vec) VALUES (?1, ?2, ?3, ?4, ?5, ?6)")
        .map_err(|e| e.to_string())?;
    let mut written = 0u64;
    for (p, mut v) in pending.drain(..).zip(vectors) {
        match dim {
            Some(d) if *d != v.len() => {
                return Err(format!(
                    "Модель вернула вектор размерности {}, а индекс строится с {d}",
                    v.len()
                ));
            }
            Some(_) => {}
            None => *dim = Some(v.len()),
        }
        normalize(&mut v);
        let (scale, q) = quantize(&v);
        let blob: Vec<u8> = q.iter().map(|x| *x as u8).collect();
        stmt.execute(params![
            p.file_id,
            p.chunk.start_line as i64,
            p.chunk.end_line as i64,
            preview_of(&p.chunk.text),
            f64::from(scale),
            blob
        ])
        .map_err(|e| e.to_string())?;
        written += 1;
    }
    Ok(written)
}

/// Counts what `build_vector_index` would embed, without touching Ollama or the index.
pub fn plan_vector_index(env: &DriveEnv, repo_filter: Option<&str>) -> Result<VectorProgress, String> {
    let vault = env.vault_dir();
    if !vault.is_dir() {
        return Err(format!("Папка git_vault не найдена: {}", vault.display()));
    }
    let mut prog = VectorProgress::default();
    for f in walk_vault(&vault, repo_filter, &mut prog.skipped) {
        prog.files += 1;
        prog.chunks += chunk_lines(&f.text, WINDOW_LINES, STRIDE_LINES).len() as u64;
    }
    Ok(prog)
}

/// Builds or refreshes `<root>/vectordb/vectors.sqlite`. Files whose content hash is unchanged
/// keep their vectors; a different embedding model wipes the index first. With `repo_filter`
/// only that repository is (re)embedded and files of other repositories are left alone.
pub fn build_vector_index(
    env: &DriveEnv,
    embedder: &dyn Embedder,
    repo_filter: Option<&str>,
    progress: &mut dyn FnMut(&VectorProgress),
) -> Result<VectorSummary, String> {
    let vault = env.vault_dir();
    if !vault.is_dir() {
        return Err(format!("Папка git_vault не найдена: {}", vault.display()));
    }
    let path = env.vectors_path();
    fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| format!("Не удалось создать папку векторного индекса: {e}"))?;
    let started = Instant::now();
    let conn = open_rw(&path)?;
    if let Some(prev) = meta_get(&conn, "model")
        && prev != embedder.model()
    {
        conn.execute_batch("DELETE FROM chunks; DELETE FROM files; DELETE FROM meta;")
            .map_err(|e| e.to_string())?;
    }
    let mut dim: Option<usize> = meta_get(&conn, "dim").and_then(|d| d.parse().ok());

    let mut existing: HashMap<String, (i64, i64, u64)> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, path, hash, chunks FROM files")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)? as u64,
                ))
            })
            .map_err(|e| e.to_string())?;
        for row in rows.flatten() {
            existing.insert(row.1, (row.0, row.2, row.3));
        }
    }

    let mut prog = VectorProgress::default();
    let files = walk_vault(&vault, repo_filter, &mut prog.skipped);
    let mut seen: HashSet<String> = HashSet::new();
    let mut pending: Vec<Pending> = Vec::new();
    conn.execute_batch("BEGIN").map_err(|e| e.to_string())?;
    for f in files {
        prog.files += 1;
        seen.insert(f.rel.clone());
        if let Some((_, hash, chunks)) = existing.get(&f.rel)
            && *hash == f.hash
        {
            prog.chunks += chunks;
            prog.reused += chunks;
            continue;
        }
        if let Some((id, _, _)) = existing.get(&f.rel) {
            conn.execute("DELETE FROM chunks WHERE file_id = ?1", params![id])
                .map_err(|e| e.to_string())?;
            conn.execute("DELETE FROM files WHERE id = ?1", params![id])
                .map_err(|e| e.to_string())?;
        }
        let chunks = chunk_lines(&f.text, WINDOW_LINES, STRIDE_LINES);
        conn.execute(
            "INSERT INTO files(path, repo, hash, chunks) VALUES (?1, ?2, ?3, ?4)",
            params![f.rel, f.repo, f.hash, chunks.len() as i64],
        )
        .map_err(|e| e.to_string())?;
        let file_id = conn.last_insert_rowid();
        prog.chunks += chunks.len() as u64;
        for chunk in chunks {
            pending.push(Pending { file_id, chunk });
            if pending.len() >= BATCH {
                prog.embedded += flush_batch(&conn, embedder, &mut pending, &mut dim)?;
                progress(&prog);
            }
        }
        if prog.files % 50 == 0 {
            progress(&prog);
        }
    }
    prog.embedded += flush_batch(&conn, embedder, &mut pending, &mut dim)?;
    // Files that disappeared from the (filtered) vault.
    for (rel, (id, _, _)) in &existing {
        if seen.contains(rel) || !repo_matches(&repo_of(rel), repo_filter) {
            continue;
        }
        conn.execute("DELETE FROM chunks WHERE file_id = ?1", params![id])
            .map_err(|e| e.to_string())?;
        conn.execute("DELETE FROM files WHERE id = ?1", params![id])
            .map_err(|e| e.to_string())?;
    }
    let total_files: u64 = conn
        .query_row("SELECT COUNT(*) FROM files", [], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())? as u64;
    let total_chunks: u64 = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |r| r.get::<_, i64>(0))
        .map_err(|e| e.to_string())? as u64;
    let dim_value = dim.unwrap_or(0);
    meta_set(&conn, "model", embedder.model())?;
    meta_set(&conn, "dim", &dim_value.to_string())?;
    meta_set(&conn, "files", &total_files.to_string())?;
    meta_set(&conn, "chunks", &total_chunks.to_string())?;
    meta_set(&conn, "built_at", &now_iso())?;
    meta_set(&conn, "root", &vault.display().to_string())?;
    meta_set(&conn, "window", &WINDOW_LINES.to_string())?;
    meta_set(&conn, "stride", &STRIDE_LINES.to_string())?;
    conn.execute_batch("COMMIT").map_err(|e| e.to_string())?;
    drop(conn);
    invalidate_cache(&path);
    progress(&prog);
    Ok(VectorSummary {
        path: path.display().to_string(),
        model: embedder.model().to_string(),
        dim: dim_value,
        files: total_files,
        chunks: total_chunks,
        embedded: prog.embedded,
        reused: prog.reused,
        skipped: prog.skipped,
        elapsed_ms: started.elapsed().as_millis() as u64,
    })
}

pub fn index_info(env: &DriveEnv) -> Option<VectorIndexInfo> {
    let path = env.vectors_path();
    let meta = fs::metadata(&path).ok()?;
    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY).ok();
    let get = |k: &str| conn.as_ref().and_then(|c| meta_get(c, k));
    Some(VectorIndexInfo {
        path: path.display().to_string(),
        bytes: meta.len(),
        model: get("model"),
        dim: get("dim").and_then(|v| v.parse().ok()),
        files: get("files").and_then(|v| v.parse().ok()),
        chunks: get("chunks").and_then(|v| v.parse().ok()),
        built_at: get("built_at"),
    })
}

// ---------------------------------------------------------------------------------------------
// Searching
// ---------------------------------------------------------------------------------------------

struct Row {
    file: u32,
    start_line: u32,
    end_line: u32,
    scale: f32,
    vec: Vec<i8>,
    preview: String,
}

struct Cache {
    path: PathBuf,
    modified: Option<SystemTime>,
    len: u64,
    dim: usize,
    files: Vec<(String, String)>,
    rows: Vec<Row>,
}

static CACHE: Mutex<Option<Arc<Cache>>> = Mutex::new(None);

fn invalidate_cache(path: &Path) {
    if let Ok(mut guard) = CACHE.lock()
        && guard.as_ref().is_some_and(|c| c.path == path)
    {
        *guard = None;
    }
}

/// The whole index in memory, reloaded when the file on disk changes.
fn load_cache(path: &Path) -> Result<Arc<Cache>, String> {
    let meta = fs::metadata(path).map_err(|e| format!("Векторный индекс не читается: {e}"))?;
    let modified = meta.modified().ok();
    let len = meta.len();
    if let Ok(guard) = CACHE.lock()
        && let Some(c) = guard.as_ref()
        && c.path == path
        && c.modified == modified
        && c.len == len
    {
        return Ok(Arc::clone(c));
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("Не удалось открыть векторный индекс: {e}"))?;
    let dim: usize = meta_get(&conn, "dim")
        .and_then(|d| d.parse().ok())
        .unwrap_or(0);
    let mut files = Vec::new();
    let mut file_index: HashMap<i64, u32> = HashMap::new();
    {
        let mut stmt = conn
            .prepare("SELECT id, path, repo FROM files ORDER BY id")
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?))
            })
            .map_err(|e| e.to_string())?;
        for (id, path, repo) in rows.flatten() {
            file_index.insert(id, files.len() as u32);
            files.push((path, repo));
        }
    }
    let mut rows = Vec::new();
    {
        let mut stmt = conn
            .prepare("SELECT file_id, start_line, end_line, scale, vec, preview FROM chunks ORDER BY id")
            .map_err(|e| e.to_string())?;
        let iter = stmt
            .query_map([], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, f64>(3)?,
                    r.get::<_, Vec<u8>>(4)?,
                    r.get::<_, String>(5)?,
                ))
            })
            .map_err(|e| e.to_string())?;
        for (file_id, start, end, scale, blob, preview) in iter.flatten() {
            let Some(file) = file_index.get(&file_id) else { continue };
            if blob.len() != dim {
                continue;
            }
            rows.push(Row {
                file: *file,
                start_line: start as u32,
                end_line: end as u32,
                scale: scale as f32,
                vec: blob.iter().map(|b| *b as i8).collect(),
                preview,
            });
        }
    }
    let cache = Arc::new(Cache {
        path: path.to_path_buf(),
        modified,
        len,
        dim,
        files,
        rows,
    });
    if let Ok(mut guard) = CACHE.lock() {
        *guard = Some(Arc::clone(&cache));
    }
    Ok(cache)
}

/// Reciprocal rank fusion of several ranked lists of keys; higher is better.
pub fn rrf<'a>(lists: &[Vec<&'a str>]) -> Vec<(&'a str, f32)> {
    let mut scores: HashMap<&str, f32> = HashMap::new();
    let mut order: Vec<&str> = Vec::new();
    for list in lists {
        for (rank, key) in list.iter().enumerate() {
            let entry = scores.entry(key).or_insert_with(|| {
                order.push(key);
                0.0
            });
            *entry += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
    }
    let mut out: Vec<(&str, f32)> = order.into_iter().map(|k| (k, scores[k])).collect();
    out.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// Files ranked by meaning (best window per file), optionally fused with the FTS5 ranking.
pub fn semantic_search(
    env: &DriveEnv,
    embedder: &dyn Embedder,
    query: &str,
    repo: Option<&str>,
    limit: usize,
    hybrid: bool,
) -> Result<Vec<SemanticHit>, String> {
    let path = env.vectors_path();
    if !path.is_file() {
        return Err("Векторный индекс ещё не построен: в приложении «Диск знаний» → «Построить векторный индекс» (или `codewhale-desktop mcp-drive embed`).".into());
    }
    let query = query.trim();
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let cache = load_cache(&path)?;
    if cache.rows.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = embedder
        .embed(&[query.to_string()], true)?
        .pop()
        .ok_or("Модель не вернула вектор запроса")?;
    if q.len() != cache.dim {
        return Err(format!(
            "Размерность вектора запроса ({}) не совпадает с индексом ({}): индекс построен моделью {}, а запрос считает {}. Перестройте индекс.",
            q.len(),
            cache.dim,
            index_info(env).and_then(|i| i.model).unwrap_or_default(),
            embedder.model()
        ));
    }
    normalize(&mut q);
    let (q_scale, q_vec) = quantize(&q);

    // Best window per file.
    let mut best: HashMap<u32, (f32, usize)> = HashMap::new();
    for (i, row) in cache.rows.iter().enumerate() {
        if !repo_matches(&cache.files[row.file as usize].1, repo) {
            continue;
        }
        let sim = cosine_q8(q_scale, &q_vec, row.scale, &row.vec);
        let entry = best.entry(row.file).or_insert((f32::MIN, i));
        if sim > entry.0 {
            *entry = (sim, i);
        }
    }
    let mut by_vector: Vec<(u32, f32, usize)> = best.into_iter().map(|(f, (s, i))| (f, s, i)).collect();
    by_vector.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    by_vector.truncate(limit.max(1) * 3);

    let mut vector_hits: HashMap<&str, (u32, f32, usize)> = HashMap::new();
    let vector_keys: Vec<&str> = by_vector
        .iter()
        .map(|(f, s, i)| {
            let key = cache.files[*f as usize].0.as_str();
            vector_hits.insert(key, (*f, *s, *i));
            key
        })
        .collect();

    let fts_hits: Vec<drive_mcp::CodeHit> = if hybrid {
        drive_mcp::code_search(env, query, repo, limit.max(1) * 3).unwrap_or_default()
    } else {
        Vec::new()
    };
    let fts_keys: Vec<&str> = fts_hits.iter().map(|h| h.path.as_str()).collect();
    let fts_by_path: HashMap<&str, &drive_mcp::CodeHit> =
        fts_hits.iter().map(|h| (h.path.as_str(), h)).collect();

    let fused = if hybrid {
        rrf(&[vector_keys, fts_keys])
    } else {
        rrf(&[vector_keys])
    };
    let mut out = Vec::new();
    for (key, _) in fused.into_iter().take(limit.max(1)) {
        let vec_hit = vector_hits.get(key);
        let fts_hit = fts_by_path.get(key);
        let source = match (vec_hit.is_some(), fts_hit.is_some()) {
            (true, true) => "both",
            (true, false) => "vector",
            _ => "fts",
        };
        match (vec_hit, fts_hit) {
            (Some((f, sim, i)), _) => {
                let row = &cache.rows[*i];
                let (path, repo) = &cache.files[*f as usize];
                out.push(SemanticHit {
                    path: path.clone(),
                    repo: repo.clone(),
                    start_line: row.start_line as usize,
                    end_line: row.end_line as usize,
                    similarity: Some((sim * 1000.0).round() / 1000.0),
                    source: source.into(),
                    preview: row.preview.clone(),
                });
            }
            (None, Some(h)) => out.push(SemanticHit {
                path: h.path.clone(),
                repo: h.repo.clone(),
                start_line: 0,
                end_line: 0,
                similarity: None,
                source: source.into(),
                preview: h.snippet.clone(),
            }),
            (None, None) => {}
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Bag-of-words embedder: every word hashes into one of 64 buckets, so texts sharing words
    /// get similar vectors. Deterministic and offline.
    pub struct FakeEmbedder {
        pub name: String,
        pub dim: usize,
    }

    impl Default for FakeEmbedder {
        fn default() -> Self {
            Self { name: "fake-embed".into(), dim: 64 }
        }
    }

    impl Embedder for FakeEmbedder {
        fn model(&self) -> &str {
            &self.name
        }
        fn embed(&self, inputs: &[String], _query: bool) -> Result<Vec<Vec<f32>>, String> {
            Ok(inputs
                .iter()
                .map(|text| {
                    let mut v = vec![0.0f32; self.dim];
                    for word in text
                        .split(|c: char| !c.is_alphanumeric())
                        .filter(|w| w.len() > 2)
                    {
                        let h = fnv1a64(word.to_ascii_lowercase().as_bytes());
                        v[(h % self.dim as u64) as usize] += 1.0;
                    }
                    v
                })
                .collect())
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-drive-vec-{tag}-{}-{}",
            std::process::id(),
            now_iso().replace(':', "-")
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("git_vault/gamedev/demo")).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join("git_vault").join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn numbered(prefix: &str, n: usize) -> String {
        (1..=n)
            .map(|i| format!("{prefix} line number {i} with enough words to count"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn chunks_overlap_and_cover_the_file() {
        let text = numbered("code", 100);
        let chunks = chunk_lines(&text, 40, 32);
        assert_eq!(chunks.len(), 3);
        assert_eq!((chunks[0].start_line, chunks[0].end_line), (1, 40));
        assert_eq!((chunks[1].start_line, chunks[1].end_line), (33, 72));
        assert_eq!((chunks[2].start_line, chunks[2].end_line), (65, 100));
        assert!(chunks[0].text.starts_with("code line number 1 "));
        assert!(chunks[2].text.ends_with("line number 100 with enough words to count"));
    }

    #[test]
    fn tiny_and_empty_texts_give_no_chunks() {
        assert!(chunk_lines("", 40, 32).is_empty());
        assert!(chunk_lines("x\n\ny\n", 40, 32).is_empty());
        let one = chunk_lines(&numbered("short", 5), 40, 32);
        assert_eq!(one.len(), 1);
        assert_eq!((one[0].start_line, one[0].end_line), (1, 5));
    }

    #[test]
    fn long_lines_are_clipped_and_stride_never_exceeds_window() {
        let long = "я".repeat(1000);
        let chunks = chunk_lines(&long, 40, 100);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].text.chars().count(), MAX_LINE_CHARS);
    }

    #[test]
    fn quantised_cosine_matches_float_cosine() {
        let mut a: Vec<f32> = (0..768).map(|i| ((i * 7) % 13) as f32 - 6.0).collect();
        let mut b: Vec<f32> = (0..768).map(|i| ((i * 5) % 11) as f32 - 5.0).collect();
        normalize(&mut a);
        normalize(&mut b);
        let exact: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
        let (sa, qa) = quantize(&a);
        let (sb, qb) = quantize(&b);
        let approx = cosine_q8(sa, &qa, sb, &qb);
        assert!((approx - exact).abs() < 0.02, "{approx} vs {exact}");
        assert!((cosine_q8(sa, &qa, sa, &qa) - 1.0).abs() < 0.02);
    }

    #[test]
    fn rrf_prefers_items_present_in_both_lists() {
        let fused = rrf(&[vec!["a", "b", "c"], vec!["c", "d"]]);
        assert_eq!(fused[0].0, "c");
        assert_eq!(fused[1].0, "a");
        assert_eq!(fused.len(), 4);
    }

    #[test]
    fn build_search_and_incremental_rebuild() {
        let root = temp_root("build");
        write(&root, "gamedev/demo/water.gd", &numbered("gerstner wave crest foam ocean", 50));
        write(&root, "gamedev/demo/player.gd", &numbered("jump velocity gravity player", 50));
        write(&root, "gamedev/demo/icon.png", "not text");
        let env = DriveEnv::new(&root);
        let fake = FakeEmbedder::default();
        let mut reports = 0;
        let summary = build_vector_index(&env, &fake, None, &mut |_| reports += 1).unwrap();
        assert_eq!(summary.files, 2);
        assert_eq!(summary.chunks, 4);
        assert_eq!(summary.embedded, 4);
        assert_eq!(summary.reused, 0);
        assert_eq!(summary.dim, 64);
        assert!(reports >= 1);
        let info = index_info(&env).unwrap();
        assert_eq!(info.model.as_deref(), Some("fake-embed"));
        assert_eq!(info.chunks, Some(4));

        let hits = semantic_search(&env, &fake, "ocean wave with foam", None, 5, false).unwrap();
        assert_eq!(hits[0].path, "gamedev/demo/water.gd");
        assert_eq!(hits[0].source, "vector");
        assert!(hits[0].similarity.unwrap() > hits[1].similarity.unwrap());
        assert_eq!(hits[0].start_line, 1);
        assert!(hits[0].preview.contains("gerstner"));
        assert!(semantic_search(&env, &fake, "jump", Some("gamedev/demo"), 5, false).unwrap()[0]
            .path
            .ends_with("player.gd"));
        assert!(semantic_search(&env, &fake, "jump", Some("physics"), 5, false).unwrap().is_empty());

        // Unchanged files keep their vectors; a changed file is re-embedded; a removed one goes.
        write(&root, "gamedev/demo/player.gd", &numbered("dash sprint stamina player", 50));
        let _ = fs::remove_file(root.join("git_vault/gamedev/demo/icon.png"));
        let second = build_vector_index(&env, &fake, None, &mut |_| {}).unwrap();
        assert_eq!(second.reused, 2);
        assert_eq!(second.embedded, 2);
        assert_eq!(second.chunks, 4);
        let hits = semantic_search(&env, &fake, "sprint stamina", None, 5, false).unwrap();
        assert_eq!(hits[0].path, "gamedev/demo/player.gd");
        fs::remove_file(root.join("git_vault/gamedev/demo/player.gd")).unwrap();
        let third = build_vector_index(&env, &fake, None, &mut |_| {}).unwrap();
        assert_eq!(third.files, 1);
        assert_eq!(third.chunks, 2);

        // A different model wipes the index instead of mixing vector spaces.
        let other = FakeEmbedder { name: "other".into(), dim: 32 };
        let fourth = build_vector_index(&env, &other, None, &mut |_| {}).unwrap();
        assert_eq!(fourth.dim, 32);
        assert_eq!(fourth.reused, 0);
        let err = semantic_search(&env, &fake, "wave", None, 5, false).unwrap_err();
        assert!(err.contains("Размерность"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn hybrid_search_fuses_with_fts() {
        let root = temp_root("hybrid");
        write(&root, "gamedev/demo/water.gd", &numbered("gerstner wave crest foam ocean", 50));
        write(&root, "gamedev/demo/buoy.gd", &numbered("apply_buoyancy_force float submerged", 50));
        let env = DriveEnv::new(&root);
        let fake = FakeEmbedder::default();
        build_vector_index(&env, &fake, None, &mut |_| {}).unwrap();
        drive_mcp::build_index(&env, &mut |_| {}).unwrap();
        // The identifier only matches through FTS (the fake embedder splits on `_`, so the
        // exact token never meets the query), the meaning only through vectors.
        let hits = semantic_search(&env, &fake, "apply_buoyancy_force", None, 5, true).unwrap();
        assert_eq!(hits[0].path, "gamedev/demo/buoy.gd");
        assert!(hits.iter().any(|h| h.source == "both" || h.source == "fts"));
        let hits = semantic_search(&env, &fake, "foam on the crest of the wave", None, 5, true).unwrap();
        assert_eq!(hits[0].path, "gamedev/demo/water.gd");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_index_explains_how_to_build_it() {
        let root = temp_root("missing");
        let env = DriveEnv::new(&root);
        let err = semantic_search(&env, &FakeEmbedder::default(), "wave", None, 5, true).unwrap_err();
        assert!(err.contains("mcp-drive embed"));
        assert!(index_info(&env).is_none());
        let plan = plan_vector_index(&env, None).unwrap();
        assert_eq!(plan.files, 0);
        let _ = fs::remove_dir_all(&root);
    }
}
