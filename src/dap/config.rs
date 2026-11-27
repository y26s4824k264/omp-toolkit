//! DAP debug adapter catalog backed by Oh My Pi v19.1.0's `dap/defaults.json`
//! plus optional project overrides (`.omp/dap.json`, `.dap.json`, `dap.json`).

use std::{
    collections::BTreeMap,
    env, fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

const DEFAULTS_JSON: &str = include_str!("../../assets/dap-defaults.json");

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterConfig {
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub connect_mode: Option<String>,
    #[serde(default)]
    pub languages: Vec<String>,
    #[serde(default)]
    pub file_types: Vec<String>,
    #[serde(default)]
    pub root_markers: Vec<String>,
    #[serde(default)]
    pub accepts_directory_program: Option<bool>,
    #[serde(default)]
    pub launch_defaults: Option<Value>,
    #[serde(default)]
    pub attach_defaults: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ResolvedAdapter {
    pub id: String,
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub is_socket: bool,
    pub launch_defaults: Map<String, Value>,
    pub attach_defaults: Map<String, Value>,
}

#[derive(Debug, Deserialize)]
struct ProjectDapFile {
    #[serde(default)]
    adapters: BTreeMap<String, Value>,
}

/// Load built-in OMP DAP adapter configs merged with project-local overrides.
pub fn load_adapters(root: &Path) -> Result<BTreeMap<String, AdapterConfig>> {
    let mut map: BTreeMap<String, AdapterConfig> =
        serde_json::from_str(DEFAULTS_JSON).context("parse bundled dap-defaults.json")?;

    for rel in [".omp/dap.json", ".dap.json", "dap.json"] {
        let path = root.join(rel);
        if !path.is_file() {
            continue;
        }
        let raw = fs::read_to_string(&path)
            .with_context(|| format!("read DAP config {}", path.display()))?;
        let parsed: ProjectDapFile = serde_json::from_str(&raw)
            .with_context(|| format!("parse DAP config {}", path.display()))?;
        for (id, overlay) in parsed.adapters {
            let mut merged = map
                .get(&id)
                .and_then(|existing| serde_json::to_value(existing).ok())
                .unwrap_or_else(|| Value::Object(Map::new()));
            merge_json(&mut merged, overlay);
            let cfg: AdapterConfig = serde_json::from_value(merged)
                .with_context(|| format!("invalid adapter '{id}' in {}", path.display()))?;
            map.insert(id, cfg);
        }
    }
    Ok(map)
}

/// List all configured adapters along with whether they are installed locally.
pub fn list_adapters(root: &Path) -> Result<Vec<Value>> {
    let map = load_adapters(root)?;
    Ok(map
        .into_iter()
        .map(|(id, cfg)| {
            let bin = find_adapter_binary(&id, &cfg.command);
            serde_json::json!({
                "id": id,
                "command": cfg.command,
                "available": bin.is_some(),
                "resolved_binary": bin.map(|p| p.to_string_lossy().into_owned()),
                "connect_mode": cfg.connect_mode.unwrap_or_else(|| "stdio".into()),
                "languages": cfg.languages,
                "file_types": cfg.file_types,
            })
        })
        .collect())
}

/// Resolve an adapter by explicit ID or infer from `program` path & locally installed tools.
pub fn resolve_adapter(
    root: &Path,
    requested: Option<&str>,
    program: Option<&Path>,
) -> Result<ResolvedAdapter> {
    let map = load_adapters(root)?;
    if let Some(id) = requested.map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(cfg) = map.get(id) {
            let binary = find_adapter_binary(id, &cfg.command).with_context(|| {
                format!("debug adapter '{id}' (command `{}`) was not found on PATH", cfg.command)
            })?;
            return Ok(build_resolved(id.to_owned(), binary, cfg));
        }
        // Allow specifying a custom adapter binary directly.
        if let Some(binary) = find_adapter_binary(id, id) {
            return Ok(ResolvedAdapter {
                id: id.to_owned(),
                binary,
                args: Vec::new(),
                is_socket: false,
                launch_defaults: Map::new(),
                attach_defaults: Map::new(),
            });
        }
        bail!("unknown or unavailable debug adapter '{id}'");
    }

    // Auto-select based on target program extension or directory.
    let mut preferred_ids: Vec<&str> = Vec::new();
    if let Some(prog) = program {
        if prog.is_dir() {
            preferred_ids.push("dlv");
        } else {
            let ext = prog
                .extension()
                .and_then(|e| e.to_str())
                .map(str::to_ascii_lowercase)
                .unwrap_or_default();
            match ext.as_str() {
                "py" => preferred_ids.push("debugpy"),
                "go" => preferred_ids.push("dlv"),
                "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => {
                    preferred_ids.push("js-debug-adapter");
                }
                "rb" => preferred_ids.push("rdbg"),
                "dart" => preferred_ids.push("dart-debug-adapter"),
                "sh" | "bash" => preferred_ids.push("bash-debug-adapter"),
                _ => {
                    preferred_ids.extend(["lldb-dap", "codelldb", "gdb"]);
                }
            }
        }
    } else {
        preferred_ids.extend(["lldb-dap", "codelldb", "gdb", "debugpy", "dlv"]);
    }

    for id in &preferred_ids {
        if let Some(cfg) = map.get(*id)
            && let Some(binary) = find_adapter_binary(id, &cfg.command)
        {
            return Ok(build_resolved((*id).to_owned(), binary, cfg));
        }
    }

    bail!(
        "no installed debug adapter found (tried: {}); pass `adapter` explicitly or install lldb-dap / gdb / debugpy / dlv",
        preferred_ids.join(", ")
    )
}

fn build_resolved(id: String, binary: PathBuf, cfg: &AdapterConfig) -> ResolvedAdapter {
    let is_socket = cfg.connect_mode.as_deref() == Some("socket")
        || cfg.args.iter().any(|a| a == "--port");
    let launch_defaults = cfg
        .launch_defaults
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let attach_defaults = cfg
        .attach_defaults
        .as_ref()
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    ResolvedAdapter {
        id,
        binary,
        args: cfg.args.clone(),
        is_socket,
        launch_defaults,
        attach_defaults,
    }
}

fn find_adapter_binary(id: &str, command: &str) -> Option<PathBuf> {
    if id == "debugpy" {
        for py in ["python3", "python"] {
            if let Some(bin) = find_on_path(py) {
                return Some(bin);
            }
        }
        return None;
    }
    if let Some(found) = find_on_path(command) {
        return Some(found);
    }
    #[cfg(target_os = "macos")]
    if id == "lldb-dap" || command == "lldb-dap" {
        if let Ok(output) = std::process::Command::new("xcrun")
            .args(["--find", "lldb-dap"])
            .output()
            && output.status.success()
        {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            let path = PathBuf::from(text);
            if path.is_file() {
                return Some(path);
            }
        }
    }
    None
}

fn find_on_path(command: &str) -> Option<PathBuf> {
    let p = Path::new(command);
    if p.is_absolute() {
        return p.is_file().then(|| p.to_path_buf());
    }
    let path_env = env::var_os("PATH")?;
    for dir in env::split_paths(&path_env) {
        #[cfg(windows)]
        {
            for ext in ["exe", "cmd", "bat"] {
                let with_ext = dir.join(format!("{command}.{ext}"));
                if with_ext.is_file() {
                    return Some(with_ext);
                }
            }
            let candidate = dir.join(command);
            if command.contains('.') && candidate.is_file() {
                return Some(candidate);
            }
        }
        #[cfg(not(windows))]
        {
            let candidate = dir.join(command);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn merge_json(base: &mut Value, overlay: Value) {
    match (base, overlay) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                merge_json(b.entry(k).or_insert(Value::Null), v);
            }
        }
        (slot, val) => *slot = val,
    }
}
