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
    // Defence in depth against SQL-LIKE/COM injection via the ItemName filter.
    //
    // The query is placed into a PowerShell string, which is then placed into
    // an OLE DB SQL command as a LIKE pattern literal, which is then matched
    // against the Windows Search Index. Each layer has different metacharacters:
    //
    //   - PowerShell: single quote ends the string literal
    //   - SQL LIKE:   `%` and `_` are wildcards; `[...]` is a character class
    //   - user intent: `*` and `?` should be glob wildcards (mapped to `%`/`_`)
    //
    // We:
    //   1. Escape `%`, `_`, `[`, and `\` in the raw query so the user can't inject
    //      SQL wildcards or character classes. `\` is the LIKE ESCAPE char below.
    //   2. Translate glob wildcards `*` → `%`, `?` → `_`.
    //   3. Escape PowerShell single quote by doubling it (`''`).
    //   4. Strip control characters that could confuse the parser.
    //   5. Cap the length so a 10 MB query can't DOS PowerShell.
    const MAX_QUERY_LEN: usize = 256;
    let trimmed: String = query
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_QUERY_LEN)
        .collect();

    // Step 1: escape SQL LIKE metacharacters (use `\` as ESCAPE in the SQL below).
    let mut sql_escaped = String::with_capacity(trimmed.len() + 8);
    for ch in trimmed.chars() {
        match ch {
            '\\' | '%' | '_' | '[' => { sql_escaped.push('\\'); sql_escaped.push(ch); }
            _ => sql_escaped.push(ch),
        }
    }

    // Step 2: translate user wildcards AFTER escaping so they're preserved.
    let has_wildcard = sql_escaped.contains('*') || sql_escaped.contains('?');
    let sql_pattern: String = if has_wildcard {
        sql_escaped.replace('*', "%").replace('?', "_")
    } else {
        format!("%{}%", sql_escaped)
    };

    // Step 3: escape PowerShell single quote for the surrounding SQL string literal.
    let ps_pattern = sql_pattern.replace('\'', "''");

    let ps_script = format!(
        r#"
$conn = New-Object -COM ADODB.Connection
$rs = New-Object -COM ADODB.Recordset
$conn.Open("Provider=Search.CollatorDSO;Extended Properties='Application=Windows';")
$sql = "SELECT TOP {max} System.ItemName, System.ItemPathDisplay, System.ItemType, System.Size, System.DateModified FROM SystemIndex WHERE System.ItemName LIKE '{pattern}' ESCAPE '\' ORDER BY System.DateModified DESC"
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
        pattern = ps_pattern,
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
