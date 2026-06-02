use serde_json::{json, Value};

// Notebook Python-cell execution (a managed kernel with persistent state) isn't
// ported; report no output and an explanatory error.
#[tauri::command]
pub fn notebook_run_python_cell() -> Value {
    json!({
        "stdout": "",
        "stderr": "",
        "exitCode": null,
        "error": "Notebook execution isn't available in this build."
    })
}
