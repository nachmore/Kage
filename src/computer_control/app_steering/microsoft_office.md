# Microsoft Office App Steering

## Launching Office Apps

IMPORTANT: To skip the start/backstage screen, launch Office apps with these commands:
- Word: `winword.exe /w` (opens directly to a blank document)
- Excel: `excel.exe /e` (opens directly to a blank workbook)
- PowerPoint: `powerpnt.exe /s` (opens directly to a blank presentation)

When using launch_and_get_tree(), use these executable names with switches:
- launch_and_get_tree("winword /w") for Word
- launch_and_get_tree("excel /e") for Excel
- launch_and_get_tree("powerpnt") for PowerPoint

## Handling the Backstage/Start Screen

If the app opens to the backstage view (start screen with templates and recent files):

1. Look for the "Backstage view" pane in the UI tree — this confirms you're on the start screen
2. Find the "Blank document" (Word), "Blank workbook" (Excel), or "Blank Presentation" (PowerPoint) listitem
3. It will have actions=[invoke,select] — use click_element() to click it
4. After clicking, call get_ui_tree() to verify the blank document is now active
5. The document area should show a [document] element — that's your editing surface

## Verifying a Blank Document is Ready

A blank document is ready when:
- The UI tree shows a [document] element (not hidden behind backstage)
- The title bar shows something like "Document1 - Word" (not just "Word")
- There is NO "Backstage view" pane visible

If the backstage view is still showing after clicking "Blank document", try pressing Escape (key_press("escape")) to dismiss it.

## Typing into Documents

IMPORTANT: Use `focus_element()` instead of `click_element()` when you need to type into a document.
- `focus_element(element_id)` sets keyboard focus via the accessibility API without moving the mouse
- Then use `type_text("your text")` to type — keystrokes go to the focused element
- This is more reliable than clicking because the user's mouse position is not disturbed

Example workflow:
1. Find the [document] or [edit] element in the UI tree
2. `focus_element(element_id)` to set focus
3. `type_text("hello")` to type

## Common Gotchas

- Office apps take 3-5 seconds to fully load — use wait_seconds=4.0 in launch_and_get_tree
- "What's New" or update dialogs may appear — dismiss them with Escape or by clicking Close
- The ribbon may take a moment to load — verify it's visible before interacting
- If Word shows "Confidential" in the title bar, that's normal for enterprise installations
