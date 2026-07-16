//! Automation plans, inline-assist, and script/macro execution.

use super::*;

/// Execute an automation plan step by step using sub-agents.
/// Each step is executed in a fresh sub-agent context, keeping the main
/// session clean and avoiding context window bloat.
///
/// The plan is a JSON array of steps, each with "step", "task", and "details" fields.
/// Progress events are emitted to the frontend as each step completes.
#[tauri::command]
pub async fn execute_automation_plan(
    session_id: String,
    plan_json: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
    window: WebviewWindow,
) -> Result<(), AppError> {
    info!("Executing automation plan");

    // Parse the plan
    let plan: Vec<serde_json::Value> = serde_json::from_str(&plan_json)
        .map_err(|_| AppError::keyed(ErrorKind::Internal, "errors.plan.invalid_json", &[]))?;

    if plan.is_empty() {
        return Err(AppError::keyed(
            ErrorKind::Internal,
            "errors.plan.empty",
            &[],
        ));
    }

    let total_steps = plan.len();
    info!("Plan has {} steps", total_steps);

    // Emit plan start event back to the calling window only.
    crate::event_targets::emit_to_self(
        &window,
        "automation_plan_start",
        &serde_json::json!({
            "totalSteps": total_steps,
            "plan": plan,
        }),
    );

    let client = acp.client.clone();
    let cancelled = features.automation_plan_cancelled.clone();

    // Reset cancellation flag at the start
    cancelled.store(false, std::sync::atomic::Ordering::Relaxed);

    async_runtime::spawn_blocking(move || {
        if !client.is_connected() {
            if let Err(e) = client.connect() {
                crate::event_targets::emit_to_self(
                    &window,
                    "automation_plan_error",
                    &format!("Unable to connect: {}", e),
                );
                return;
            }
        }

        for (i, step) in plan.iter().enumerate() {
            // Check cancellation before starting each step
            if cancelled.load(std::sync::atomic::Ordering::Relaxed) {
                info!("Automation plan cancelled by user at step {}", i + 1);
                break;
            }

            let step_num = i + 1;
            let task = step
                .get("task")
                .and_then(|t| t.as_str())
                .unwrap_or("Unknown task");
            let details = step.get("details").and_then(|d| d.as_str()).unwrap_or("");

            info!("Executing step {}/{}: {}", step_num, total_steps, task);

            // Emit step start event
            crate::event_targets::emit_to_self(
                &window,
                "automation_step_start",
                &serde_json::json!({
                    "step": step_num,
                    "totalSteps": total_steps,
                    "task": task,
                    "details": details,
                }),
            );

            // Build the sub-agent query with full context
            let query = format!(
                "You are a UI automation sub-agent. Execute this specific task:\n\n\
                 Task: {}\n\
                 Details: {}\n\n\
                 RULES:\n\
                 1. FIRST: Call get_app_steering(task='{}', details='{}') for app-specific tips.\n\
                 2. Use computer-control MCP tools (prefer compound tools like \
                 launch_and_get_tree, click_and_get_tree, click_and_read_result).\n\
                 3. NEVER use screenshot() — use get_ui_tree() or find_elements() instead.\n\
                 4. You MUST call at least one tool. Do NOT claim success without tool evidence.\n\
                 5. Report the ACTUAL tool output. If a tool returns an error, report the error.\n\
                 6. Do NOT fabricate or hallucinate results. Only report what tools actually returned.\n\
                 7. If the task fails, say FAILED and explain why with the actual error message.\n\
                 8. Be concise — just report what happened.",
                task, details, task, details
            );

            // Invoke the sub-agent
            match client.invoke_subagent(&session_id, &query) {
                Ok(result) => {
                    info!(
                        "Step {}/{} completed: {} chars",
                        step_num,
                        total_steps,
                        result.len()
                    );

                    // Check if the sub-agent reported a failure in its response text.
                    // The ACP call succeeded (we got a response), but the agent may
                    // have said "FAILED" because it couldn't actually perform the task.
                    let result_lower = result.to_lowercase();
                    let agent_reported_failure = result_lower.starts_with("failed")
                        || result_lower.contains("\nfailed")
                        || result_lower.contains("failed —")
                        || result_lower.contains("failed -");

                    if agent_reported_failure {
                        warn!(
                            "Step {}/{} agent reported failure: {}",
                            step_num,
                            total_steps,
                            result.chars().take(200).collect::<String>()
                        );
                    }

                    let success = !agent_reported_failure;

                    crate::event_targets::emit_to_self(
                        &window,
                        events::AUTOMATION_STEP_COMPLETE,
                        &serde_json::json!({
                            "step": step_num,
                            "totalSteps": total_steps,
                            "task": task,
                            "result": result,
                            "success": success,
                        }),
                    );

                    if !success {
                        warn!(
                            "Aborting automation plan: step {}/{} failed",
                            step_num, total_steps
                        );
                        // Mark remaining steps as stopped
                        for (j, remaining) in plan.iter().enumerate().skip(i + 1) {
                            let remaining_task = remaining
                                .get("task")
                                .and_then(|t| t.as_str())
                                .unwrap_or("Unknown task");
                            crate::event_targets::emit_to_self(
                                &window,
                                events::AUTOMATION_STEP_COMPLETE,
                                &serde_json::json!({
                                    "step": j + 1,
                                    "totalSteps": total_steps,
                                    "task": remaining_task,
                                    "result": "Skipped due to earlier step failure",
                                    "success": false,
                                    "stopped": true,
                                }),
                            );
                        }
                        break;
                    }
                }
                Err(e) => {
                    let error_msg = format!("{}", e);
                    warn!("Step {}/{} failed: {}", step_num, total_steps, error_msg);

                    crate::event_targets::emit_to_self(
                        &window,
                        events::AUTOMATION_STEP_COMPLETE,
                        &serde_json::json!({
                            "step": step_num,
                            "totalSteps": total_steps,
                            "task": task,
                            "result": error_msg,
                            "success": false,
                        }),
                    );

                    // Abort on transport/protocol errors too
                    warn!(
                        "Aborting automation plan: step {}/{} errored",
                        step_num, total_steps
                    );
                    for (j, remaining) in plan.iter().enumerate().skip(i + 1) {
                        let remaining_task = remaining
                            .get("task")
                            .and_then(|t| t.as_str())
                            .unwrap_or("Unknown task");
                        crate::event_targets::emit_to_self(
                            &window,
                            events::AUTOMATION_STEP_COMPLETE,
                            &serde_json::json!({
                                "step": j + 1,
                                "totalSteps": total_steps,
                                "task": remaining_task,
                                "result": "Skipped due to earlier step failure",
                                "success": false,
                                "stopped": true,
                            }),
                        );
                    }
                    break;
                }
            }
        }

        // Emit plan complete event
        crate::event_targets::emit_to_self(
            &window,
            "automation_plan_complete",
            &serde_json::json!({
                "totalSteps": total_steps,
            }),
        );

        // MESSAGE_COMPLETE is what closes out the streaming UI in
        // chat-host windows — emit there regardless of which window
        // started the plan, otherwise a plan kicked off from the
        // floating launcher leaves the chat row in "streaming" state.
        let app = window.app_handle().clone();
        crate::event_targets::emit_to_chat_hosts(&app, events::MESSAGE_COMPLETE, &());
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Inline Assist messaging
// ---------------------------------------------------------------------------

/// Send a message for inline assist and stream the response to the
/// inline-assist window. The frontend passes the session id to use
/// (typically the floating window's session).
#[tauri::command]
pub async fn send_inline_assist(
    session_id: Option<String>,
    message: String,
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle,
) -> Result<(), AppError> {
    let client = acp.client.clone();

    async_runtime::spawn_blocking(move || {
        if !client.is_connected() {
            if let Err(e) = client.connect() {
                crate::event_targets::emit_to_inline_assist(
                    &app,
                    events::INLINE_ASSIST_ERROR,
                    &format!("Unable to connect: {}", e),
                );
                return;
            }
        }

        // Resolve a real session — the floating window may not have
        // pinned one yet (inline-assist is hotkey-driven and can fire
        // before the floating UI was ever opened).
        let session_id = match resolve_or_create_session(&client, session_id) {
            Ok(id) => id,
            Err(e) => {
                crate::event_targets::emit_to_inline_assist(
                    &app,
                    events::INLINE_ASSIST_ERROR,
                    &format!("Failed: {}", e),
                );
                return;
            }
        };

        // send_chat_streaming resets its own session bucket; once it
        // returns, the response is available in that bucket.
        if let Err(e) = client.send_chat_streaming(&session_id, &message, None) {
            crate::event_targets::emit_to_inline_assist(
                &app,
                events::INLINE_ASSIST_ERROR,
                &format!("Failed: {}", e),
            );
            return;
        }

        let result = client.take_session_accumulator(&session_id);
        if result.trim().is_empty() {
            crate::event_targets::emit_to_inline_assist(
                &app,
                events::INLINE_ASSIST_ERROR,
                &"Empty response",
            );
        } else {
            crate::event_targets::emit_to_inline_assist(&app, "inline_assist_chunk", &result);
            crate::event_targets::emit_to_inline_assist(&app, "inline_assist_complete", &());
        }
    });

    Ok(())
}

// ---------------------------------------------------------------------------
// Script generation — one-shot, ephemeral session
// ---------------------------------------------------------------------------

/// Generate a script from a natural-language prompt and return the
/// agent's full response text.
///
/// Runs on a throwaway [`crate::ephemeral_session`] rather than a real
/// chat session: the script editor lives in Settings, which is frequently
/// open with no chat window present — so there's no session to borrow (the
/// old frontend passed `get_window_session(main)`, which is `null` in that
/// case and crashed `send_message_streaming`'s `sessionId: String`
/// argument). Borrowing a real session would also inject the generation
/// prompt and its reply into the user's actual conversation history.
#[tauri::command]
pub async fn generate_script(
    prompt: String,
    acp: State<'_, AcpHandles>,
    features: State<'_, FeatureServices>,
) -> Result<String, AppError> {
    let client = acp.client.clone();
    let config = features.config.clone();

    async_runtime::spawn_blocking(move || {
        crate::ephemeral_session::prompt_once(&client, &config, &prompt).map_err(|e| {
            AppError::keyed(
                ErrorKind::Internal,
                "errors.script.generate_failed",
                &[("reason", &e.to_string())],
            )
        })
    })
    .await
    .map_err(|e| AppError::internal(format!("generate_script task panicked: {}", e)))?
}

// ---------------------------------------------------------------------------
// Macro execution — chained transformation steps (AI, regex, transform, script)
// ---------------------------------------------------------------------------

/// Execute a macro: run each step sequentially, feeding output into the next.
/// Steps can be AI prompts, find/replace, built-in transforms, or JS scripts.
/// Returns the final result text. The caller passes the session id that
/// AI-prompt steps should land on (typically the calling window's
/// pinned session).
#[tauri::command]
pub async fn execute_macro(
    session_id: Option<String>,
    steps: Vec<serde_json::Value>,
    initial_input: String,
    acp: State<'_, AcpHandles>,
    app: tauri::AppHandle,
) -> Result<String, AppError> {
    let client = acp.client.clone();
    let step_count = steps.len();
    // Cloned for the post-await telemetry track. Used to also be used
    // by a per-step `macro_progress` emit, but that event has no
    // frontend listener so the per-step broadcast was deleted.
    let app_for_event = app.clone();
    let result = async_runtime::spawn_blocking(move || -> Result<String, AppError> {
        let mut current_input = initial_input;
        // Resolve a real session up front — the inline-assist macro path
        // borrows the floating window's session, which may be unpinned.
        // Only ai_prompt steps actually need it, but resolving once here
        // keeps the per-step code simple and the cost is one create at
        // most (skipped entirely when the caller passed a live id).
        let session_id = resolve_or_create_session(&client, session_id)?;

        for (i, step) in steps.iter().enumerate() {
            let step_type = step.get("step_type").and_then(|v| v.as_str()).unwrap_or("ai_prompt");

            // (Was: emit `macro_progress` here. No frontend listener
            // ever subscribed, so it was a per-step broadcast doing
            // nothing. Drop the emit instead of paying the eval cost
            // on every step.)

            match step_type {
                "ai_prompt" => {
                    let prompt_template = step.get("prompt").and_then(|v| v.as_str()).unwrap_or("{input}");
                    let prompt = prompt_template.replace("{input}", &current_input);
                    let full_prompt = format!(
                        "{}\n\n[_KAGE_INLINE] Return ONLY the result text. No explanations, no markdown formatting, no code fences.",
                        prompt
                    );

                    if !client.is_connected() {
                        if let Err(e) = client.connect() {
                            return Err(AppError::keyed(
                                ErrorKind::ConnectionLost,
                                "errors.macro.connect_failed",
                                &[
                                    ("step", &(i + 1).to_string()),
                                    ("reason", &e.to_string()),
                                ],
                            ));
                        }
                    }
                    if let Err(e) = client.send_chat_streaming(&session_id, &full_prompt, None) {
                        return Err(AppError::keyed(
                            ErrorKind::Internal,
                            "errors.macro.step_failed",
                            &[
                                ("step", &(i + 1).to_string()),
                                ("reason", &e.to_string()),
                            ],
                        ));
                    }
                    let result = client.take_session_accumulator(&session_id);
                    if result.trim().is_empty() {
                        return Err(AppError::keyed(
                            ErrorKind::Internal,
                            "errors.macro.step_empty_result",
                            &[("step", &(i + 1).to_string())],
                        ));
                    }
                    current_input = result.trim().to_string();
                }

                "find_replace" => {
                    let find = step.get("find").and_then(|v| v.as_str()).unwrap_or("");
                    let replace = step.get("replace").and_then(|v| v.as_str()).unwrap_or("");
                    if !find.is_empty() {
                        match regex::Regex::new(find) {
                            Ok(re) => {
                                current_input = re.replace_all(&current_input, replace).to_string();
                            }
                            Err(e) => {
                                return Err(AppError::keyed(
                                    ErrorKind::Internal,
                                    "errors.macro.invalid_regex",
                                    &[
                                        ("step", &(i + 1).to_string()),
                                        ("pattern", find),
                                        ("reason", &e.to_string()),
                                    ],
                                ));
                            }
                        }
                    }
                }

                "transform" => {
                    let transform = step.get("transform").and_then(|v| v.as_str()).unwrap_or("");
                    current_input = apply_transform(transform, &current_input);
                }

                other => {
                    return Err(AppError::keyed(
                        ErrorKind::Internal,
                        "errors.macro.unknown_step",
                        &[("step", &(i + 1).to_string()), ("kind", other)],
                    ));
                }
            }

            info!("Macro step {}/{} ({}) complete: {} chars", i + 1, steps.len(), step_type, current_input.len());
        }

        Ok(current_input)
    })
    .await
    .map_err(|e| {
        AppError::keyed(
            ErrorKind::Internal,
            "errors.task.failed",
            &[("reason", &e.to_string())],
        )
    })?;

    // Telemetry — fire on success only. step_count is captured before
    // the move; we don't include macro names because those are user-typed.
    if result.is_ok() {
        crate::telemetry::track(
            &app_for_event,
            "macro_executed",
            Some(serde_json::json!({ "step_count": step_count })),
        );
    }

    result
}

/// Apply a built-in text transform.
fn apply_transform(name: &str, input: &str) -> String {
    match name {
        "uppercase" => input.to_uppercase(),
        "lowercase" => input.to_lowercase(),
        "trim" => input.trim().to_string(),
        "sort_lines" => {
            let mut lines: Vec<&str> = input.lines().collect();
            lines.sort();
            lines.join("\n")
        }
        "reverse" => input.chars().rev().collect(),
        "reverse_lines" => {
            let lines: Vec<&str> = input.lines().collect();
            lines.into_iter().rev().collect::<Vec<_>>().join("\n")
        }
        "remove_blank_lines" => input
            .lines()
            .filter(|l| !l.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        "count_words" => {
            let count = input.split_whitespace().count();
            format!("{} words", count)
        }
        "count_lines" => {
            let count = input.lines().count();
            format!("{} lines", count)
        }
        "count_chars" => {
            format!("{} characters", input.len())
        }
        "base64_encode" => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(input.as_bytes())
        }
        "base64_decode" => {
            use base64::Engine;
            match base64::engine::general_purpose::STANDARD.decode(input.trim()) {
                Ok(bytes) => String::from_utf8_lossy(&bytes).to_string(),
                Err(e) => format!("Base64 decode error: {}", e),
            }
        }
        "unique_lines" => {
            let mut seen = std::collections::HashSet::new();
            input
                .lines()
                .filter(|l| seen.insert(*l))
                .collect::<Vec<_>>()
                .join("\n")
        }
        "number_lines" => input
            .lines()
            .enumerate()
            .map(|(i, l)| format!("{:>4}  {}", i + 1, l))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => input.to_string(),
    }
}
