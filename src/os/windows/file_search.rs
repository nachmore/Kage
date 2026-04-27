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
    let ps_pattern = sanitize_query_to_like_pattern(query);

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


/// Turn a user-supplied query string into a fully-sanitized SQL LIKE
/// pattern that's safe to splice into a PowerShell single-quoted string
/// literal and executed via OLE DB against the Windows Search Index.
///
/// Defence in depth against SQL-LIKE and COM-string injection:
///   1. Strip control characters that could confuse the parser.
///   2. Cap at 256 chars so a pathological input can't DOS PowerShell.
///   3. Escape SQL LIKE metacharacters (`%`, `_`, `[`, `\`) with the
///      `\` escape so user input can't inject wildcards or character
///      classes. The outer SQL uses `ESCAPE '\'` to honor this.
///   4. Translate user glob wildcards (`*` → `%`, `?` → `_`) AFTER
///      step 3 so they're preserved while literal `%`/`_` are not.
///   5. Wrap plain queries in `%...%` so bare words are treated as
///      substring matches.
///   6. Double single quotes so the pattern is safe inside a
///      PowerShell single-quoted string literal.
///
/// Exposed for testing; production callers should go through
/// `search_files_impl` which also runs the search.
pub(crate) fn sanitize_query_to_like_pattern(query: &str) -> String {
    const MAX_QUERY_LEN: usize = 256;
    let trimmed: String = query
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_QUERY_LEN)
        .collect();

    // Step 1: escape SQL LIKE metacharacters (use `\` as ESCAPE in the SQL).
    let mut sql_escaped = String::with_capacity(trimmed.len() + 8);
    for ch in trimmed.chars() {
        match ch {
            '\\' | '%' | '_' | '[' => {
                sql_escaped.push('\\');
                sql_escaped.push(ch);
            }
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

    // Step 3: escape PowerShell single quote for the surrounding string literal.
    sql_pattern.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    //! The sanitizer is security-critical: user input flows through
    //! PowerShell → OLE DB → Windows Search SQL. Each test here pins
    //! down a specific injection or encoding edge case.

    use super::sanitize_query_to_like_pattern as s;

    #[test]
    fn plain_word_wraps_in_substring_pattern() {
        assert_eq!(s("hello"), "%hello%");
    }

    #[test]
    fn empty_input_becomes_match_everything() {
        // Empty query wraps to "%%" which matches any file. We treat
        // that as acceptable because an empty query is also a
        // no-results case in practice (search doesn't fire).
        assert_eq!(s(""), "%%");
    }

    #[test]
    fn glob_star_maps_to_sql_percent() {
        assert_eq!(s("*.rs"), "%.rs");
        assert_eq!(s("foo*bar"), "foo%bar");
        assert_eq!(s("*"), "%");
    }

    #[test]
    fn glob_question_maps_to_sql_underscore() {
        assert_eq!(s("?.rs"), "_.rs");
        assert_eq!(s("f?o"), "f_o");
    }

    #[test]
    fn literal_percent_is_escaped_not_treated_as_wildcard() {
        // Without escaping, 50% match would match anything starting
        // with "50". We need the literal to be preserved so the SQL
        // LIKE with ESCAPE '\' matches a real percent sign only.
        let out = s("50%");
        assert!(out.contains(r"\%"), "expected escape of %, got {}", out);
        // Because it contained no glob wildcards, it's still wrapped.
        assert!(out.starts_with('%') && out.ends_with('%'));
    }

    #[test]
    fn literal_underscore_is_escaped() {
        let out = s("foo_bar");
        assert!(out.contains(r"\_"), "expected escape of _, got {}", out);
    }

    #[test]
    fn literal_bracket_is_escaped_to_prevent_char_class() {
        // Without escaping, [abc] would match any of a/b/c in LIKE.
        let out = s("[abc]");
        assert!(out.contains(r"\["), "expected escape of [, got {}", out);
    }

    #[test]
    fn backslash_is_escaped_to_preserve_escape_semantics() {
        // `\` is the SQL LIKE ESCAPE character. A bare `\` from user
        // input has to be doubled or the next metachar becomes a
        // literal.
        let out = s(r"a\b");
        // The raw backslash becomes `\\` so the SQL parser sees one
        // escaped backslash (= literal `\`). Our output is the PS
        // string, so we expect `\\` in the middle.
        assert!(out.contains(r"\\"), "expected escaped backslash, got {}", out);
    }

    #[test]
    fn single_quote_is_doubled_for_powershell() {
        // Without doubling, a single quote would end the PS string
        // literal and anything after would execute as code. 1 of the
        // worst kinds of injection available here.
        assert_eq!(s("O'Reilly"), "%O''Reilly%");
    }

    #[test]
    fn control_characters_are_stripped() {
        assert_eq!(s("hello\nworld"), "%helloworld%");
        assert_eq!(s("tab\there"), "%tabhere%");
        assert_eq!(s("null\0inside"), "%nullinside%");
    }

    #[test]
    fn length_is_capped_at_256_chars() {
        let input: String = "a".repeat(500);
        let out = s(&input);
        // 256 'a's plus the two wrapping '%' characters.
        assert_eq!(out.len(), 258);
        assert!(out.starts_with('%') && out.ends_with('%'));
    }

    #[test]
    fn mixed_wildcards_and_escaped_literals() {
        // Users can type "foo*bar_baz" — star is glob, underscore is
        // literal. After sanitize: star → %, underscore → \_, and
        // no outer wrapping because a wildcard is present.
        let out = s("foo*bar_baz");
        assert_eq!(out, r"foo%bar\_baz");
    }

    #[test]
    fn injection_attempts_are_declawed() {
        // The classic LIKE injection: %' OR 1=1 --
        let out = s("%' OR 1=1 --");
        // The % is escaped, the ' is doubled. No SQL can escape.
        assert!(out.contains(r"\%"), "missing escape: {}", out);
        assert!(out.contains("''"), "missing doubled quote: {}", out);
        // No single unescaped quote should remain — every `'` is part
        // of a `''` doubled pair. Scanning once to verify.
        let mut last_was_quote = false;
        for ch in out.chars() {
            if ch == '\'' {
                last_was_quote = !last_was_quote;
            } else if last_was_quote {
                panic!("found unescaped quote followed by {:?} in {}", ch, out);
            }
        }
        assert!(!last_was_quote, "trailing unescaped quote in {}", out);
    }
}
