//! Run the etylizer type checker as a subprocess and turn its JSON diagnostics into ELP
//! diagnostics.
//!
//! etylizer is a standalone Erlang escript that parses `.erl` files itself and emits a JSON
//! array of diagnostics on stdout (one object per type error), exiting 0 even when there are
//! diagnostics. See `etylizer --report-mode json`

use std::path::Path;
use std::process::Command;

use anyhow::Result;
use elp_ide::TextRange;
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

/// Path to the etylizer escript. Override with `ELP_ETYLIZER_PATH`; defaults to `etylizer` on `PATH`.
fn etylizer_path() -> String {
    std::env::var("ELP_ETYLIZER_PATH").unwrap_or_else(|_| "etylizer".to_string())
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
    let mut cmd = Command::new(etylizer_path());
    cmd.args(["--report-mode", "json", "--no-deps", "-P"])
        .arg(root);
    // Pass the app's include directories so etylizer can resolve `-include(...)`; without
    // these, any file with an include fails to parse and etylizer reports nothing.
    for inc in include_dirs {
        cmd.arg("-I").arg(inc);
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
/// FIXME #112 source ranges will fix this, currently its kept as a point diagnostic
pub fn to_diagnostic(line_index: &LineIndex, d: &EtylizerDiagnostic) -> Option<Diagnostic> {
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
    Some(Diagnostic::new(
        DiagnosticCode::Etylizer(d.kind.clone()),
        d.message.clone(),
        TextRange::empty(offset),
    ))
}
