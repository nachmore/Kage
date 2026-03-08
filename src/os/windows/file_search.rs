// Windows file search using the Windows Search Index.
//
// Queries the SystemIndex catalog via OLE DB (through a small PowerShell helper).
// The index query itself is instant — the overhead is PowerShell startup (~200ms).
// Results are cached per-session to avoid repeated startup costs.
//
// Future: can be extended to support Everything SDK for even faster results.

use log::{info, warn};
use std::process::{Command, Stdio};

use crate::os::file_search::FileSearchResult;

/// Search the Windows Search Index for files matching the query.
pub fn search_files_impl(query: &str, max_results: usize) -> Vec<FileSearchResult> {
    let safe_query = query.replace('\'', "''");

    // Use OLE DB via PowerShell to query the Windows Search Index.
    // This is fast because the index is pre-built — only PowerShell startup adds latency.
    // Translate user wildcards to SQL LIKE patterns
    // *.png → search by extension, hello* → prefix match, etc.
    let has_wildcard = safe_query.contains('*') || safe_query.contains('?');
    let like_pattern = if has_wildcard {
        safe_query.replace('*', "%").replace('?', "_")
    } else {
        format!("%{}%", safe_query)
    };

    let ps_script = format!(
        r#"
$conn = New-Object -COM ADODB.Connection
$rs = New-Object -COM ADODB.Recordset
$conn.Open("Provider=Search.CollatorDSO;Extended Properties='Application=Windows';")
$sql = "SELECT TOP {max} System.ItemName, System.ItemPathDisplay, System.ItemType, System.Size, System.DateModified FROM SystemIndex WHERE System.ItemName LIKE '{pattern}' ORDER BY System.DateModified DESC"
$rs.Open($sql, $conn)
$results = @()
while (-not $rs.EOF) {{
    $results += [PSCustomObject]@{{
        name = [string]$rs.Fields.Item("System.ItemName").Value
        path = [string]$rs.Fields.Item("System.ItemPathDisplay").Value
        item_type = [string]$rs.Fields.Item("System.ItemType").Value
        size = if ($rs.Fields.Item("System.Size").Value) {{ [long]$rs.Fields.Item("System.Size").Value }} else {{ 0 }}
        modified = if ($rs.Fields.Item("System.DateModified").Value) {{ ([datetime]$rs.Fields.Item("System.DateModified").Value).ToString("o") }} else {{ "" }}
    }}
    $rs.MoveNext()
}}
$rs.Close()
$conn.Close()
$results | ConvertTo-Json -Compress
"#,
        max = max_results,
        pattern = like_pattern,
    );

    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", &ps_script])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let output = match cmd.output() {
        Ok(o) => o,
        Err(e) => {
            warn!("[file_search] Failed to run PowerShell: {}", e);
            return vec![];
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        return vec![];
    }

    // PowerShell returns a single object (not array) when there's only one result
    let parsed: Vec<PsSearchResult> = if stdout.trim().starts_with('[') {
        serde_json::from_str(&stdout).unwrap_or_default()
    } else {
        serde_json::from_str::<PsSearchResult>(&stdout)
            .map(|r| vec![r])
            .unwrap_or_default()
    };

    info!("[file_search] Found {} results for '{}'", parsed.len(), query);

    parsed.into_iter().map(|r| {
        let is_folder = r.item_type.is_empty() || r.item_type == "Directory";
        FileSearchResult {
            name: r.name,
            path: r.path,
            is_folder,
            size: r.size,
            modified: r.modified,
        }
    }).collect()
}

#[derive(serde::Deserialize, Default)]
struct PsSearchResult {
    #[serde(default)]
    name: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    item_type: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    modified: String,
}
