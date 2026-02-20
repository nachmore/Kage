# Kiro Assistant UX Improvements

## Summary
Streamlined the UX to match the Kiro desktop experience with a cleaner, more compact design. Added draggable floating window with animated ghost and smart positioning.

## Changes Made

### 1. Floating Window (ui/floating.html)

#### Layout Changes
- **Ghost position**: Moved to the left side of the input box (horizontal layout)
- **Reduced padding**: Minimized padding throughout for a more compact design
- **Ghost size**: Reduced from 80px circle to 60px x full-height rectangle
- **Container width**: Set to 500px fixed width
- **Removed hint text**: Eliminated "Press Enter to chat • Esc to dismiss"

#### New Features
- **Draggable window**: Click and drag the ghost to move the window anywhere on screen
- **Smart positioning**: Window appears 1/3 from top of screen by default (requires backend)
- **Animated ghost**: Bounces and pulses while generating responses
- **Inline response display**: Shows answer directly in the floating window
- **Content area**: New scrollable area for displaying responses
- **Expand button**: Icon button to open full chat experience
- **Loading indicator**: Animated dots while waiting for response
- **Textarea input**: Changed from input to textarea with auto-resize (max 100px)
- **Single Q&A mode**: Only shows current question/answer, previous messages hidden

#### Ghost Animations
1. **Thinking state**: 
   - Ghost bounces up and down (8px movement)
   - Container pulses with glowing shadow
   - Both animations run at 1-2 second intervals
2. **Idle state**: Static ghost, ready for input
3. **Cursor**: Changes to "move" cursor when hovering over ghost

#### Behavior
1. User asks a question
2. Ghost starts bouncing and pulsing
3. Loading dots appear
4. Response shows in content area
5. Ghost stops animating
6. Expand button appears to open full chat
7. Next question clears previous response (session continues in background)

### 2. Theme System

#### Settings (ui/settings.html)
Added theme selector with three options:
- **System (Auto)** - Default, follows OS theme
- **Dark** - Always dark theme
- **Light** - Always light theme

#### Implementation
- Detects system theme preference using `prefers-color-scheme`
- Listens for system theme changes in real-time
- Applies theme immediately on save
- Theme persists across sessions

#### Dark Theme Colors
Based on Kiro desktop "Prey" palette:
- Background: `#211D25` (prey-800)
- Panel: `#28242E` (prey-750)
- Surface: `#352F3D` (prey-700)
- Border: `#4A464F` (prey-600)
- Text: `#E5E7EB` (light gray)
- Muted text: `#938F9B` (prey-400)
- Accent: `#C09CFF` (light purple)

### 3. Visual Refinements

#### Floating Window
- Ghost container: Rounded left corners only (`border-radius: 12px 0 0 12px`)
- Speech bubble: Rounded right corners only (`border-radius: 0 12px 12px 0`)
- Removed speech bubble arrow
- Input has no border, just bottom border on container
- Expand button: Subtle icon, appears only when response is shown

#### All Windows
- Consistent dark theme support
- Smooth transitions between themes
- Proper contrast ratios for accessibility
- Gradient backgrounds adjusted for dark mode

### 4. Interaction Improvements

#### Floating Window
- **Shift+Enter**: New line in textarea
- **Enter**: Send message
- **Esc**: Close window (existing)
- **Arrow keys**: Navigate app suggestions (existing)
- **Expand button**: Opens full chat window with conversation history

#### Response Flow
```
User Input → Loading → Response Display → Expand Option
     ↓
Next Question → Clears Display → New Response
     ↓
(Session continues, history preserved in backend)
```

## Files Modified

1. **ui/floating.html**
   - Complete layout restructure
   - Added response display area
   - Added expand button
   - Added loading indicator
   - Removed hint text
   - Added dark theme styles
   - Added ghost bounce animation
   - Added container pulse animation
   - Added drag cursor on ghost
   - Added drag event handlers
   - Added thinking state management

2. **ui/settings.html**
   - Added "System (Auto)" theme option
   - Added theme description text
   - Added theme application logic
   - Added dark theme styles
   - Added system theme change listener

3. **ui/index.html**
   - Added dark theme styles
   - Added theme detection logic
   - Added system theme change listener

## Testing Checklist

- [ ] Floating window appears 1/3 from top, centered horizontally
- [ ] Floating window appears with ghost on left
- [ ] Input box takes up most of the space
- [ ] No hint text visible
- [ ] Ghost shows move cursor on hover
- [ ] Can click and drag ghost to move window
- [ ] Window stays in new position after dragging
- [ ] Asking a question shows ghost bouncing animation
- [ ] Ghost container pulses with glow during thinking
- [ ] Loading dots appear while thinking
- [ ] Ghost stops animating when response arrives
- [ ] Response appears in content area
- [ ] Expand button appears after response
- [ ] Clicking expand opens full chat
- [ ] Next question clears previous response
- [ ] Theme setting defaults to "System (Auto)"
- [ ] Dark theme applies correctly
- [ ] Light theme applies correctly
- [ ] System theme changes are detected
- [ ] Theme persists after restart
- [ ] Window position persists after restart (optional)

## Next Steps (Backend Integration Needed)

1. **Window positioning**: Set default position to 1/3 from top of screen (see BACKEND_CHANGES_NEEDED.md)
2. **Window dragging**: Implement `start_drag_window` command (see BACKEND_CHANGES_NEEDED.md)
3. **Position persistence**: Optionally save window position between sessions
4. **Floating window response**: Connect to actual backend for responses
5. **Session management**: Maintain conversation history in backend
6. **Expand functionality**: Pass full conversation to main window via `open_chat_window` command
7. **Theme persistence**: Save theme preference to config file
8. **Theme application**: Apply saved theme on startup

See `BACKEND_CHANGES_NEEDED.md` for detailed Rust implementation examples.
