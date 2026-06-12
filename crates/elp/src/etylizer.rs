//! Run the etylizer type checker as a subprocess and turn its JSON diagnostics into ELP
//! diagnostics.
//!
//! etylizer is a standalone Erlang escript that parses `.erl` files itself and emits a JSON
//! array of diagnostics on stdout (one object per type error), exiting 0 even when there are
//! diagnostics. See `etylizer --report-mode json`

use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

use anyhow::Result;
use elp_ide::TextRange;
use elp_ide::TextSize;
use elp_ide::diagnostics::Diagnostic;
use elp_ide::diagnostics::DiagnosticCode;
use elp_ide::elp_ide_db::LineCol;
use elp_ide::elp_ide_db::LineIndex;
use elp_ide::elp_ide_db::elp_base_db::AbsPath;
use serde::Deserialize;

/// The json-mode result of an etylizer run
/// Etylizer currently uses its own cache.
/// A file present in `checked` had all of its currently-failing functions
/// re-checked, so its diagnostics are complete and can replace the cache. 
/// A file absent from `checked` was skipped (unchanged) and its cached diagnostics must be kept.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EtylizerOutput {
    #[serde(default)]
    pub checked: Vec<String>,
    #[serde(default)]
    pub diagnostics: Vec<EtylizerDiagnostic>,
}

/// One diagnostic as emitted by `etylizer --report-mode json`. 
/// Unknown fields (e.g. `severity`, `function`, `arity`) are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct EtylizerDiagnostic {
    #[allow(dead_code)]
    pub file: String,
    /// 1-based line; `null` only for synthetic locations (filtered out by etylizer).
    pub line: Option<u32>,
    /// 1-based column; `null` when unknown.
    pub column: Option<u32>,
    /// Coarse error kind, e.g. "ty_error", "name_error".
    pub kind: String,
    pub message: String,
}

/// The etylizer escript, embedded at build time when `ELP_ETYLIZER_ESCRIPT` was set (see erlang_service escript). 
/// Empty when not bundled, we fall back to `ELP_ETYLIZER_PATH` / `etylizer` on `PATH`.
static EMBEDDED_ETYLIZER: &[u8] = include_bytes!(env!("ELP_ETYLIZER_ESCRIPT_PATH"));

/// Builds the base command to invoke etylizer. 
fn etylizer_command() -> Command {
    if let Some(path) = std::env::var_os("ELP_ETYLIZER_PATH") {
        return Command::new(path);
    }
    if let Some(escript_file) = bundled_escript_path() {
        let escript = std::env::var("ELP_ESCRIPT").unwrap_or_else(|_| "escript".to_string());
        let mut cmd = Command::new(escript);
        cmd.arg(escript_file);
        return cmd;
    }
    Command::new("etylizer")
}

/// Extracts the embedded etylizer escript to a temp file once (kept for the process lifetime,
/// since etylizer is spawned on every save) and returns its path. `None` when nothing was bundled.
fn bundled_escript_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        if EMBEDDED_ETYLIZER.is_empty() {
            return None;
        }
        let mut file = tempfile::Builder::new()
            .prefix("elp-etylizer")
            .tempfile()
            .ok()?;
        file.write_all(EMBEDDED_ETYLIZER).ok()?;
        // Keep the file alive for the whole process; don't delete it on drop.
        let temp_path = file.into_temp_path();
        let owned = temp_path.to_path_buf();
        std::mem::forget(temp_path);
        Some(owned)
    })
    .clone()
}

/// The native `espresso` binary (Berkeley logic minimizer) etylizer shells out to at runtime,
/// embedded at build time when `ELP_ETYLIZER_ESPRESSO` was set. Empty when not bundled; etylizer
/// then falls back to its own `~/.cache/etylizer/espresso`, populated by a native etylizer build.
static EMBEDDED_ESPRESSO: &[u8] = include_bytes!(env!("ELP_ETYLIZER_ESPRESSO_PATH"));

/// Extracts the embedded espresso binary to an *executable* temp file once (kept for the process
/// lifetime) and returns its path. `None` when nothing was bundled.
fn bundled_espresso_path() -> Option<PathBuf> {
    static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
    PATH.get_or_init(|| {
        if EMBEDDED_ESPRESSO.is_empty() {
            return None;
        }
        let mut builder = tempfile::Builder::new();
        builder.prefix("elp-espresso");
        // etylizer runs espresso via spawn_executable; on Windows that wants an `.exe`,
        // on unix it needs the execute bit (set below).
        #[cfg(windows)]
        builder.suffix(".exe");
        let mut file = builder.tempfile().ok()?;
        file.write_all(EMBEDDED_ESPRESSO).ok()?;
        file.flush().ok()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(file.path(), std::fs::Permissions::from_mode(0o755)).ok()?;
        }
        // Keep the file alive for the whole process; don't delete it on drop.
        let temp_path = file.into_temp_path();
        let owned = temp_path.to_path_buf();
        std::mem::forget(temp_path);
        Some(owned)
    })
    .clone()
}

/// Type-overlay files auto-discovered in `PROJECT_ROOT/overlays/*.erl`, sorted for determinism.
/// Empty when there is no `overlays/` directory.
fn discover_overlays(project_root: &Path) -> Vec<PathBuf> {
    let dir = project_root.join("overlays");
    let mut overlays: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(entries) => entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file() && p.extension().is_some_and(|e| e == "erl"))
            .collect(),
        Err(_) => Vec::new(),
    };
    overlays.sort();
    overlays
}

/// Runs etylizer on a single saved file, returning which files it (re)checked and the
/// diagnostics found.
///
/// Uses `--no-deps` (check only this file, resolving callee interfaces via the symtab) 
/// 
/// etylizer's per-function incremental check re-checks every currently-failing
/// function and skips passing/unchanged ones, so `checked` tells us whether to refresh the
/// file's diagnostics (present) or keep the cache (absent). 
/// A non-zero exit means etylizer could not check the file (e.g. a parse error, which ELP's native parser reports
/// separately); this returns an empty `EtylizerOutput` (no `checked`), so the cache is kept.
pub fn run(
    project_root: &AbsPath,
    file: &AbsPath,
    include_dirs: &[String],
) -> Result<EtylizerOutput> {
    let root: &Path = project_root.as_ref();
    let file_path: &Path = file.as_ref();
    let mut cmd = etylizer_command();
    // etylizer resolves its espresso minimizer from the ETYLIZER_ESPRESSO env var; point it at the
    // bundled per-platform binary unless the caller already set it. Without this a freshly
    // downloaded elp would depend on ~/.cache/etylizer/espresso already existing.
    if std::env::var_os("ETYLIZER_ESPRESSO").is_none() {
        if let Some(espresso) = bundled_espresso_path() {
            cmd.env("ETYLIZER_ESPRESSO", espresso);
        }
    }
    cmd.args(["--report-mode", "json", "--no-deps", "-P"])
        .arg(root);
    // Pass the app's include directories so etylizer can resolve `-include(...)`; without
    // these, any file with an include fails to parse and etylizer reports nothing.
    for inc in include_dirs {
        cmd.arg("-I").arg(inc);
    }
    // Auto-discover type overlays in PROJECT_ROOT/overlays/*.erl.
    for overlay in discover_overlays(root) {
        cmd.arg("--type-overlay").arg(overlay);
    }
    let output = cmd.arg(file_path).output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        log::info!(
            "etylizer exited with {:?} for {}: {}",
            output.status.code(),
            file_path.display(),
            stderr.trim()
        );
        return Ok(EtylizerOutput::default());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let result: EtylizerOutput = serde_json::from_str(stdout.trim())?;
    Ok(result)
}

/// Maps an etylizer diagnostic to an ELP diagnostic, converting its 1-based line/column to a
/// text range via the file's line index. Returns `None` if the diagnostic carries no usable
/// line number.
///
/// etylizer reports a single point (line/column). `range_at` widens that point to the enclosing
/// token's range so the editor draws a squiggle under the offending token; when it returns `None`
/// (no token there) we fall back to a zero-width range at the point.
/// TODO improve when #112 is merged on etylizer side
pub fn to_diagnostic(
    line_index: &LineIndex,
    range_at: impl Fn(TextSize) -> Option<TextRange>,
    d: &EtylizerDiagnostic,
) -> Option<Diagnostic> {
    let line = d.line?;
    if line == 0 {
        return None;
    }
    // etylizer columns are 1-based character columns; for ASCII this matches UTF-16.
    let col = d.column.unwrap_or(1).max(1);
    let offset = line_index.offset(LineCol {
        line: line - 1,
        col_utf16: col - 1,
    });
    let range = range_at(offset).unwrap_or_else(|| TextRange::empty(offset));
    Some(Diagnostic::new(
        DiagnosticCode::Etylizer(d.kind.clone()),
        d.message.clone(),
        range,
    ))
}
