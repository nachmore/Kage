---
inclusion: manual
---

# Claude Prompt Engineering Best Practices

Reference guide for writing and updating prompts in this codebase. Based on Anthropic's official guidance for Claude 4.x models.
Source: https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices

## Prompt Locations in This Codebase

- `src/builtin_steering.md` — Main system prompt / agent identity (compiled into binary)
- `src/auto_steering.rs` → `EXTRACTION_PROMPT` — Auto-steering preference extraction prompt
- `ui/js/settings/shortcuts.js` → `generateScript()` — Script generation prompt
- Steering delivery in `src/commands/system.rs`, `src/main.rs`, `src/commands/sessions.rs`

## Core Principles

### 1. Be Clear and Direct
- Claude follows instructions literally. Vague prompts produce vague results.
- Be specific about desired output format, constraints, tone, and audience.
- Use numbered lists or bullet points when order or completeness of steps matters.
- Golden rule: if a colleague with no context would be confused by the prompt, Claude will be too.

### 2. Provide Context / Motivation
- Explain WHY a behavior is important, not just what to do.
- Bad: `NEVER use ellipses`
- Good: `Your response will be read aloud by a text-to-speech engine, so never use ellipses since the engine won't know how to pronounce them.`
- Claude generalizes well from explanations.

### 3. Use Examples Effectively (Few-Shot)
- Examples are the most reliable way to steer output format, tone, and structure.
- Make examples relevant, diverse (cover edge cases), and structured.
- Wrap in `<example>` / `<examples>` tags to distinguish from instructions.
- 3-5 examples is the sweet spot.

### 4. Structure Prompts with XML Tags
- Use XML tags to separate instructions, context, examples, and variable inputs.
- Use consistent, descriptive tag names: `<instructions>`, `<context>`, `<input>`.
- Nest tags when content has natural hierarchy.

### 5. Give Claude a Role
- A role in the system prompt focuses behavior and tone.
- Even a single sentence makes a difference.

### 6. Long Context: Put Data First
- Place long documents/inputs near the top, above queries and instructions.
- Queries at the end can improve response quality by up to 30%.
- Use `<document>` tags with metadata for multi-document inputs.

## Output & Formatting Control

### Tell Claude What TO Do (Not What NOT To Do)
- Bad: `Do not use markdown in your response`
- Good: `Your response should be composed of smoothly flowing prose paragraphs.`
- Positive instructions are clearer and leave less room for interpretation.

### Match Prompt Style to Desired Output
- The formatting style in your prompt influences Claude's response style.
- Removing markdown from your prompt reduces markdown in the output.

## Tool Use Guidelines

### Be Explicit About Action vs. Advice
- "Can you suggest changes?" → Claude suggests. "Make these changes." → Claude acts.
- If you want implementation by default, say so in the system prompt.

### Don't Over-Emphasize Tools
- Claude 4.x is more responsive to system prompts than older models.
- Where you might have said `CRITICAL: You MUST use this tool when...`, use `Use this tool when...`
- Over-emphasis causes unnecessary tool triggering.

### Parallel Tool Calls
- Claude excels at parallel execution. Boost with explicit guidance if needed.
- Never use placeholders or guess missing parameters in tool calls.

## Thinking & Reasoning

### Avoid Overthinking Prompts
- Replace blanket defaults like "Default to using [tool]" with targeted instructions.
- Remove over-prompting. Tools that undertriggered in older models trigger appropriately now.
- Prefer general instructions over prescriptive step-by-step plans.

### Word "Think" Sensitivity
- When thinking mode is off, avoid the word "think" and variants.
- Use alternatives: consider, evaluate, analyze, assess, review, examine.

## Agentic Systems

### Autonomy & Safety Balance
- Be explicit about which actions need confirmation (destructive, irreversible, visible to others).
- Encourage local, reversible actions freely.

### Minimize Overengineering
- Claude 4.x tends to overengineer. Add guidance to keep solutions minimal when needed.
- "Only make changes that are directly requested or clearly necessary."

### Minimize Hallucinations
- Instruct Claude to read/investigate files before answering questions about them.
- "Never speculate about code you have not opened."

## Checklist for Writing/Reviewing Prompts

1. Is the desired output format explicitly specified?
2. Is there context/motivation for key instructions (the "why")?
3. Are examples provided for complex or ambiguous outputs?
4. Is content structured with XML tags where mixing instructions/context/input?
5. Are instructions framed positively (what to do, not what to avoid)?
6. Is tool guidance measured (not over-emphasized)?
7. Does the prompt style match the desired output style?
8. Are roles clearly defined?
9. For long context: is data at the top, query at the bottom?
10. Is the word "think" avoided when thinking mode may be off?
