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
    let target_is_vue = file.is_some_and(is_vue_file);
    let mut selected = Vec::new();

    for def in defs {
        let mut matches = match file {
            Some(f) => matches_file(def, f),
            None => true,
        };
        // Volar 3.x relies on `@vue/typescript-plugin` hosted inside `typescript-language-server`
        // for `.vue` script intelligence, definitions, references, diagnostics, and renames.
        if !matches
            && target_is_vue
            && def.name == "typescript-language-server"
            && find_vue_ts_plugin_dir(start_dir).is_some()
        {
            matches = true;
        }
        if !matches {
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
        let mut resolved = ResolvedServer {
            def: def.clone(),
            binary,
            args,
            workspace_root,
            marker_depth: marker_idx.unwrap_or(usize::MAX),
        };
        configure_ts_and_vue_server(&mut resolved, file);
        selected.push(resolved);
    }

    selected.sort_by(|a, b| {
        a.marker_depth
            .cmp(&b.marker_depth)
            .then_with(|| a.def.is_linter.cmp(&b.def.is_linter))
            .then_with(|| {
                if target_is_vue {
                    vue_server_rank(&a.def.name, action).cmp(&vue_server_rank(&b.def.name, action))
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .then_with(|| a.def.name.cmp(&b.def.name))
    });
    selected
}

/// Resolve the companion `typescript-language-server` instance for a `vue-language-server`
/// workspace so Volar 3.x's `tsserver/request` notifications can be forwarded via
/// `typescript.tsserverRequest` with `@vue/typescript-plugin` loaded.
pub fn resolve_vue_ts_companion(
    defs: &[ServerDef],
    root: &Path,
    workspace_root: &Path,
) -> Option<ResolvedServer> {
    let def = defs.iter().find(|d| d.name == "typescript-language-server")?;
    let dirs = ancestor_dirs(root, workspace_root);
    let binary = resolve_executable(&def.command, &dirs)?;
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
    let mut resolved = ResolvedServer {
        def: def.clone(),
        binary,
        args,
        workspace_root: workspace_root.to_path_buf(),
        marker_depth: 0,
    };
    configure_ts_and_vue_server(&mut resolved, Some(Path::new("Component.vue")));
    Some(resolved)
}

fn vue_server_rank(name: &str, action: &str) -> u8 {
    if action == "symbols" {
        match name {
            "vue-language-server" => 0,
            "typescript-language-server" => 1,
            _ => 2,
        }
    } else {
        match name {
            "typescript-language-server" => 0,
            "vue-language-server" => 1,
            _ => 2,
        }
    }
}

pub fn is_vue_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("vue"))
}

/// Locate `node_modules/typescript/lib` containing `tsserver.js`, searching upward from
/// `workspace_root` (supporting monorepos like pnpm/yarn workspaces) and falling back to
/// `omp-toolkit`'s managed LSP directory.
pub fn find_ts_lib_dir(workspace_root: &Path) -> Option<PathBuf> {
    find_upward_dir(workspace_root, &["node_modules", "typescript", "lib"], "tsserver.js")
        .or_else(|| {
            let root = crate::lsp::install::managed_lsp_root()?;
            let managed = root
                .join("npm")
                .join("node_modules")
                .join("typescript")
                .join("lib");
            managed.join("tsserver.js").is_file().then_some(managed)
        })
}

/// Locate `node_modules/@vue/typescript-plugin`, searching upward from `workspace_root`
/// and falling back to `omp-toolkit`'s managed LSP directory.
pub fn find_vue_ts_plugin_dir(workspace_root: &Path) -> Option<PathBuf> {
    find_upward_dir(
        workspace_root,
        &["node_modules", "@vue", "typescript-plugin"],
        "package.json",
    )
    .or_else(|| {
        let root = crate::lsp::install::managed_lsp_root()?;
        let managed = root
            .join("npm")
            .join("node_modules")
            .join("@vue")
            .join("typescript-plugin");
        managed.join("package.json").is_file().then_some(managed)
    })
}

/// Heuristic check for Vue projects so `typescript-language-server` loads
/// `@vue/typescript-plugin` even when inspecting `.ts` files that import `.vue` components.
pub fn is_vue_project(workspace_root: &Path, target_file: Option<&Path>) -> bool {
    if target_file.is_some_and(is_vue_file) {
        return true;
    }
    for marker in [
        "vue.config.js",
        "vue.config.ts",
        "vue.config.mjs",
        "nuxt.config.js",
        "nuxt.config.ts",
        "nuxt.config.mjs",
    ] {
        if workspace_root.join(marker).is_file() {
            return true;
        }
    }
    if let Ok(tsconfig) = fs::read_to_string(workspace_root.join("tsconfig.json"))
        && tsconfig.contains(".vue")
    {
        return true;
    }
    let mut cur = Some(workspace_root);
    for _ in 0..4 {
        let Some(dir) = cur else {
            break;
        };
        let pkg_path = dir.join("package.json");
        if let Ok(meta) = fs::metadata(&pkg_path)
            && meta.is_file()
            && meta.len() <= 256 * 1024
            && let Ok(raw) = fs::read_to_string(&pkg_path)
            && let Ok(val) = serde_json::from_str::<Value>(&raw)
            && package_json_uses_vue(&val)
        {
            return true;
        }
        cur = dir.parent();
    }
    false
}

fn package_json_uses_vue(pkg: &Value) -> bool {
    for section in ["dependencies", "devDependencies", "peerDependencies", "optionalDependencies"] {
        if let Some(obj) = pkg.get(section).and_then(Value::as_object)
            && obj
                .keys()
                .any(|k| k == "vue" || k == "nuxt" || k.starts_with("@vue/") || k == "@vitejs/plugin-vue")
        {
            return true;
        }
    }
    false
}

fn find_upward_dir(start: &Path, rel_segments: &[&str], required_file: &str) -> Option<PathBuf> {
    let mut cur = Some(start);
    for _ in 0..8 {
        let Some(dir) = cur else {
            break;
        };
        let mut candidate = dir.to_path_buf();
        for seg in rel_segments {
            candidate.push(seg);
        }
        if candidate.join(required_file).is_file() {
            return Some(candidate);
        }
        cur = dir.parent();
    }
    None
}

fn configure_ts_and_vue_server(resolved: &mut ResolvedServer, target_file: Option<&Path>) {
    let ts_lib = find_ts_lib_dir(&resolved.workspace_root);
    match resolved.def.name.as_str() {
        "typescript-language-server" => {
            if let Some(ref ts_lib_dir) = ts_lib
                && resolved.def.init_options.pointer("/tsserver/path").is_none()
            {
                merge_json_value(
                    &mut resolved.def.init_options,
                    serde_json::json!({
                        "tsserver": {
                            "path": ts_lib_dir.join("tsserver.js").to_string_lossy()
                        }
                    }),
                );
            }
            if is_vue_project(&resolved.workspace_root, target_file)
                && let Some(plugin_dir) = find_vue_ts_plugin_dir(&resolved.workspace_root)
            {
                ensure_vue_ts_plugin(&mut resolved.def.init_options, &plugin_dir);
            }
        }
        "vue-language-server" => {
            if let Some(ref ts_lib_dir) = ts_lib
                && ts_lib_dir.join("typescript.js").is_file()
                && !resolved.args.iter().any(|a| a.starts_with("--tsdk"))
            {
                resolved
                    .args
                    .push(format!("--tsdk={}", ts_lib_dir.display()));
            }
        }
        _ => {}
    }
}

fn ensure_vue_ts_plugin(init_options: &mut Value, plugin_dir: &Path) {
    if !init_options.is_object() {
        *init_options = Value::Object(Map::new());
    }
    let Some(root_obj) = init_options.as_object_mut() else {
        return;
    };
    let plugins = root_obj
        .entry("plugins")
        .or_insert_with(|| Value::Array(Vec::new()));
    if !plugins.is_array() {
        *plugins = Value::Array(Vec::new());
    }
    if let Some(arr) = plugins.as_array_mut() {
        let already = arr.iter().any(|item| {
            item.get("name")
                .and_then(Value::as_str)
                .is_some_and(|n| n == "@vue/typescript-plugin")
        });
        if !already {
            arr.push(serde_json::json!({
                "name": "@vue/typescript-plugin",
                "location": plugin_dir.to_string_lossy(),
                "languages": ["vue"],
            }));
        }
    }
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
    search_dirs.extend(crate::lsp::install::managed_lsp_bin_dirs());

    for dir in search_dirs {
        if let Some(found) = find_in_dir(&dir, command) {
            return Some(found);
        }
    }
    None
}

/// Resolve a command on system `PATH` or inside `omp-toolkit`'s managed LSP directories.
pub fn resolve_command(command: &str) -> Option<PathBuf> {
    resolve_executable(command, &[])
}

/// Locate an executable named `command` inside one directory, prioritizing Windows
/// `.exe`/`.cmd`/`.bat` shims over extensionless POSIX shell scripts.
pub fn find_in_dir(dir: &Path, command: &str) -> Option<PathBuf> {
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
        None
    }
    #[cfg(not(windows))]
    {
        let candidate = dir.join(command);
        is_executable(&candidate).then_some(candidate)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn create_fake_bin(dir: &Path, name: &str) -> Result<PathBuf> {
        fs::create_dir_all(dir)?;
        #[cfg(windows)]
        let path = dir.join(format!("{name}.cmd"));
        #[cfg(not(windows))]
        let path = dir.join(name);
        fs::write(&path, "@echo off\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs::metadata(&path)?.permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms)?;
        }
        Ok(path)
    }

    #[test]
    fn resolves_monorepo_vue_plugin_tsdk_and_server_order() -> Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().canonicalize()?;
        let frontend = root.join("frontend");
        let app = frontend.join("apps/web-app");
        let src_dir = app.join("src");
        fs::create_dir_all(&src_dir)?;

        // Place TypeScript & Vue TS plugin at the monorepo workspace root (`frontend/node_modules`).
        let ts_lib = frontend.join("node_modules/typescript/lib");
        fs::create_dir_all(&ts_lib)?;
        fs::write(ts_lib.join("tsserver.js"), "// tsserver")?;
        fs::write(ts_lib.join("typescript.js"), "// typescript")?;

        let vue_plugin = frontend.join("node_modules/@vue/typescript-plugin");
        fs::create_dir_all(&vue_plugin)?;
        fs::write(vue_plugin.join("package.json"), r#"{"name":"@vue/typescript-plugin"}"#)?;

        let bin_dir = frontend.join("node_modules/.bin");
        create_fake_bin(&bin_dir, "typescript-language-server")?;
        create_fake_bin(&bin_dir, "vue-language-server")?;

        fs::write(frontend.join("package.json"), r#"{"dependencies":{"vue":"^3.5.0"}}"#)?;
        fs::write(app.join("package.json"), r#"{"name":"web-app"}"#)?;
        fs::write(app.join("tsconfig.json"), r#"{"include":["src/**/*.ts","src/**/*.vue"]}"#)?;
        let vue_file = src_dir.join("Brand.vue");
        fs::write(&vue_file, "<script setup lang=\"ts\"></script>")?;
        let ts_file = src_dir.join("store.ts");
        fs::write(&ts_file, "export const x = 1;")?;

        assert_eq!(find_ts_lib_dir(&app), Some(ts_lib.clone()));
        assert_eq!(find_vue_ts_plugin_dir(&app), Some(vue_plugin.clone()));
        assert!(is_vue_project(&app, Some(&ts_file)));

        let defs = load_definitions(&root)?;

        // For `symbols` on `.vue`, `vue-language-server` comes first with `--tsdk`, followed by TSLS.
        let sym_servers = select_servers(&defs, &root, Some(&vue_file), "symbols");
        let sym_names: Vec<&str> = sym_servers.iter().map(|s| s.def.name.as_str()).collect();
        assert_eq!(&sym_names[..2], &["vue-language-server", "typescript-language-server"]);
        let expected_tsdk = format!("--tsdk={}", ts_lib.display());
        assert!(sym_servers[0].args.contains(&expected_tsdk));

        // For `definition` / `references` / `hover` on `.vue`, TSLS comes first with `@vue/typescript-plugin`.
        let def_servers = select_servers(&defs, &root, Some(&vue_file), "definition");
        let def_names: Vec<&str> = def_servers.iter().map(|s| s.def.name.as_str()).collect();
        assert_eq!(&def_names[..2], &["typescript-language-server", "vue-language-server"]);
        let tsls = &def_servers[0];
        assert_eq!(
            tsls.def.init_options.pointer("/tsserver/path").and_then(Value::as_str),
            Some(ts_lib.join("tsserver.js").to_string_lossy().as_ref())
        );
        let plugins = tsls.def.init_options.get("plugins").and_then(Value::as_array).unwrap();
        assert!(plugins.iter().any(|p| p["name"] == "@vue/typescript-plugin" && p["location"] == vue_plugin.to_string_lossy().as_ref()));

        // Opening a `.ts` file inside the Vue project also equips TSLS with `@vue/typescript-plugin`.
        let ts_servers = select_servers(&defs, &root, Some(&ts_file), "references");
        assert_eq!(ts_servers[0].def.name, "typescript-language-server");
        let ts_plugins = ts_servers[0].def.init_options.get("plugins").and_then(Value::as_array).unwrap();
        assert!(ts_plugins.iter().any(|p| p["name"] == "@vue/typescript-plugin"));

        let companion = resolve_vue_ts_companion(&defs, &root, &app).unwrap();
        assert_eq!(companion.def.name, "typescript-language-server");
        assert!(companion.def.init_options["plugins"].as_array().unwrap().iter().any(|p| p["name"] == "@vue/typescript-plugin"));
        Ok(())
    }
}
