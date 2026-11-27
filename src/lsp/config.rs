//! Language server catalog backed by Oh My Pi v19.1.0's `lsp/defaults.json`
//! plus optional project overrides (`.omp/lsp.json`, `.lsp.json`, `lsp.json`).

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use globset::Glob;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const DEFAULTS_JSON: &str = include_str!("../../assets/lsp-defaults.json");

const REQUIRES_MARKERS: &[&str] = &["biome", "denols", "eslint", "helm-ls", "tailwindcss"];

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawServerConfig {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub file_types: Vec<String>,
    #[serde(default)]
    pub root_markers: Vec<String>,
    #[serde(default)]
    pub init_options: Option<Value>,
    #[serde(default)]
    pub settings: Option<Value>,
    #[serde(default)]
    pub disabled: Option<bool>,
    #[serde(default)]
    pub is_linter: Option<bool>,
    #[serde(default)]
    pub language_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ServerDef {
    pub name: String,
    pub command: String,
    pub args: Vec<String>,
    pub file_types: Vec<String>,
    pub root_markers: Vec<String>,
    pub init_options: Value,
    pub settings: Value,
    pub is_linter: bool,
    pub language_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResolvedServer {
    pub def: ServerDef,
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub workspace_root: PathBuf,
    pub marker_depth: usize,
}

#[derive(Debug, Deserialize)]
struct ProjectOverrideFile {
    #[serde(default)]
    servers: BTreeMap<String, Value>,
}

/// Load built-in OMP defaults merged with any project-local LSP configuration.
pub fn load_definitions(root: &Path) -> Result<Vec<ServerDef>> {
    let mut map: BTreeMap<String, RawServerConfig> =
        serde_json::from_str(DEFAULTS_JSON).context("parse bundled lsp-defaults.json")?;

    // Enable CSS/JSON validation by default for vscode-langservers-extracted.
    if let Some(css) = map.get_mut("vscode-css-language-server") {
        merge_json_value(
            css.settings.get_or_insert_with(|| Value::Object(Map::new())),
            serde_json::json!({
                "css": { "validate": true },
                "scss": { "validate": true },
                "less": { "validate": true }
            }),
        );
    }
    if let Some(json_ls) = map.get_mut("vscode-json-language-server") {
        merge_json_value(
            json_ls
                .settings
                .get_or_insert_with(|| Value::Object(Map::new())),
            serde_json::json!({ "json": { "validate": { "enable": true } } }),
        );
    }

    for rel in [".omp/lsp.json", ".lsp.json", "lsp.json"] {
        let path = root.join(rel);
        if !path.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read LSP config {}", path.display()))?;
        let parsed: ProjectOverrideFile = serde_json::from_str(&raw)
            .with_context(|| format!("parse LSP config {}", path.display()))?;
        for (name, override_val) in parsed.servers {
            let mut merged = map
                .get(&name)
                .and_then(|existing| serde_json::to_value(existing).ok())
                .unwrap_or_else(|| Value::Object(Map::new()));
            merge_json_value(&mut merged, override_val);
            let cfg: RawServerConfig = serde_json::from_value(merged)
                .with_context(|| format!("invalid server override '{name}' in {}", path.display()))?;
            map.insert(name, cfg);
        }
    }

    let mut defs = Vec::with_capacity(map.len());
    for (name, cfg) in map {
        if cfg.disabled.unwrap_or(false) || cfg.command.trim().is_empty() {
            continue;
        }
        if !supports_stdio(&name, &cfg.command) {
            continue;
        }
        defs.push(ServerDef {
            name,
            command: cfg.command,
            args: cfg.args,
            file_types: cfg.file_types,
            root_markers: cfg.root_markers,
            init_options: cfg.init_options.unwrap_or_else(|| Value::Object(Map::new())),
            settings: cfg.settings.unwrap_or_else(|| Value::Object(Map::new())),
            is_linter: cfg.is_linter.unwrap_or(false),
            language_id: cfg.language_id,
        });
    }
    Ok(defs)
}

/// Select matching, installed language servers for an optional target file inside `root`.
pub fn select_servers(
    defs: &[ServerDef],
    root: &Path,
    file: Option<&Path>,
    action: &str,
) -> Vec<ResolvedServer> {
    let start_dir = file
        .and_then(Path::parent)
        .filter(|p| p.starts_with(root))
        .unwrap_or(root);
    let dirs = ancestor_dirs(root, start_dir);
    let mut selected = Vec::new();

    for def in defs {
        if let Some(f) = file
            && !matches_file(def, f)
        {
            continue;
        }
        if action == "diagnostics" && def.name == "emmet-language-server" {
            continue;
        }
        let marker_idx = dirs
            .iter()
            .position(|dir| def.root_markers.iter().any(|m| marker_exists(dir, m)));
        let requires_marker = file.is_none() || REQUIRES_MARKERS.contains(&def.name.as_str());
        if marker_idx.is_none() && requires_marker {
            continue;
        }
        let Some(binary) = resolve_executable(&def.command, &dirs) else {
            continue;
        };
        let workspace_root = marker_idx
            .map(|i| dirs[i].clone())
            .unwrap_or_else(|| root.to_path_buf());
        let args = def
            .args
            .iter()
            .map(|a| {
                if a == "$PID" {
                    std::process::id().to_string()
                } else {
                    a.clone()
                }
            })
            .collect();
        selected.push(ResolvedServer {
            def: def.clone(),
            binary,
            args,
            workspace_root,
            marker_depth: marker_idx.unwrap_or(usize::MAX),
        });
    }

    selected.sort_by(|a, b| {
        a.marker_depth
            .cmp(&b.marker_depth)
            .then_with(|| a.def.is_linter.cmp(&b.def.is_linter))
            .then_with(|| a.def.name.cmp(&b.def.name))
    });
    selected
}

/// Report server availability and root marker match for `status` action.
pub fn server_status(defs: &[ServerDef], root: &Path) -> Vec<Value> {
    let dirs = vec![root.to_path_buf()];
    defs.iter()
        .map(|def| {
            let binary = resolve_executable(&def.command, &dirs);
            let matches_root = def.root_markers.iter().any(|m| marker_exists(root, m));
            serde_json::json!({
                "name": def.name,
                "command": def.command,
                "available": binary.is_some(),
                "resolved_binary": binary.map(|p| p.to_string_lossy().into_owned()),
                "matches_root": matches_root,
                "is_linter": def.is_linter,
                "file_types": def.file_types,
            })
        })
        .collect()
}

/// Determine the LSP `languageId` for `file`.
pub fn language_id(def: &ServerDef, file: &Path) -> String {
    if let Some(ref id) = def.language_id {
        return id.clone();
    }
    let ext = file
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    match ext.as_str() {
        "ts" => "typescript",
        "tsx" => "typescriptreact",
        "js" | "mjs" | "cjs" => "javascript",
        "jsx" => "javascriptreact",
        "py" | "pyi" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "sh" | "bash" | "zsh" => "shellscript",
        "yml" | "yaml" => "yaml",
        "md" => "markdown",
        "h" => "c",
        "hpp" | "cc" | "cxx" | "cpp" => "cpp",
        "cs" => "csharp",
        "kt" | "kts" => "kotlin",
        other if !other.is_empty() => return other.to_owned(),
        _ => {
            return file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("plaintext")
                .to_ascii_lowercase();
        }
    }
    .to_owned()
}

fn supports_stdio(name: &str, command: &str) -> bool {
    !((name == "swiftlint" && command == "swiftlint")
        || (name == "typescript-native" && command == "tsc"))
}

fn ancestor_dirs(root: &Path, start: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = start.to_path_buf();
    loop {
        dirs.push(current.clone());
        if current == root {
            break;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    dirs
}

fn matches_file(def: &ServerDef, file: &Path) -> bool {
    let Some(name) = file.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    def.file_types.iter().any(|ft| {
        let pat = ft.to_ascii_lowercase();
        if pat.starts_with('.') {
            lower.ends_with(&pat)
        } else {
            lower == pat
        }
    })
}

fn marker_exists(dir: &Path, marker: &str) -> bool {
    if !marker.contains(['*', '?', '[', ']', '{', '}']) {
        return dir.join(marker).exists();
    }
    let Ok(glob) = Glob::new(marker) else {
        return false;
    };
    let matcher = glob.compile_matcher();
    let Ok(entries) = fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .any(|entry| entry.file_name().to_str().is_some_and(|n| matcher.is_match(n)))
}

fn resolve_executable(command: &str, ancestor_dirs: &[PathBuf]) -> Option<PathBuf> {
    let cmd_path = Path::new(command);
    if cmd_path.is_absolute() {
        return is_executable(cmd_path).then(|| cmd_path.to_path_buf());
    }
    if command.contains('/') || command.contains('\\') {
        let base = ancestor_dirs.last()?;
        let full = base.join(command);
        return is_executable(&full).then_some(full);
    }

    let mut search_dirs: Vec<PathBuf> = ancestor_dirs
        .iter()
        .map(|d| d.join("node_modules").join(".bin"))
        .collect();
    if let Some(path_env) = env::var_os("PATH") {
        search_dirs.extend(env::split_paths(&path_env));
    }

    for dir in search_dirs {
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = dir.join(format!("{command}.{ext}"));
                if is_executable(&with_ext) {
                    return Some(with_ext);
                }
            }
            let candidate = dir.join(command);
            if command.contains('.') && is_executable(&candidate) {
                return Some(candidate);
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(command);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    None
}

fn is_executable(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn merge_json_value(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(base_map), Value::Object(overlay_map)) => {
            for (k, v) in overlay_map {
                merge_json_value(base_map.entry(k).or_insert(Value::Null), v);
            }
        }
        (slot, new_val) => *slot = new_val,
    }
}
