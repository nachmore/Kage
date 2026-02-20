# Kiro Assistant UI Branding Updates

## Summary
Updated all UI files to use the official Kiro branding from kiro-desktop source code, including:
- Proper ghost SVG icons
- Brand color palette
- Gradient colors
- Animations

## Files Updated

### 1. ui/index.html (Main Chat Window)
**Changes:**
- Replaced emoji ghost (👻) with official Kiro ghost SVG in header
- Updated all ghost avatars in messages to use SVG
- Changed gradient colors from `#667eea → #764ba2` to `#8FCDFC → #C09CFF` (official Kiro gradient)
- Updated primary purple from `#667eea` to `#8E48FF` (official Kiro purple)
- Changed user message bubble gradient to match Kiro branding
- Updated loading indicator to use ghost SVG
- Applied gradient text effect to "Kiro Assistant" title

### 2. ui/floating.html (Hotkey Popup Window)
**Changes:**
- Replaced emoji ghost (👻) with official Kiro ghost SVG
- Updated ghost container gradient to use official colors
- Changed focus border color to `#8E48FF`
- Updated app icon gradient to match Kiro branding
- Changed text color on gradient backgrounds to `#19161D` for better contrast

### 3. ui/settings.html (Settings Window)
**Changes:**
- Updated background gradient to official Kiro colors
- Changed header gradient and adjusted text color for contrast
- Updated all purple accents to `#8E48FF`
- Changed button gradients to match Kiro branding
- Updated toggle switch active color
- Changed section header colors
- Updated save button gradient and text color

## Color Palette Applied

### Primary Brand Colors
- **Kiro Purple**: `#8E48FF`
- **Gradient Start**: `#8FCDFC` (light blue)
- **Gradient End**: `#C09CFF` (light purple)

### Neutral Colors (Prey Palette)
- **Dark Text**: `#19161D` (prey-900)
- **Light Background**: `#F2F1F4` (prey-100)
- **Dividers**: `#C1BEC6` (prey-300)

## Iconography

### Ghost SVG
The official Kiro ghost icon is now used throughout:
```svg
<path d="M7.58762 37.203C2.62272 48.1978 13.1975 50.9578 20.9974 44.5229..." fill="white" stroke="black"/>
<path d="M21.9284 20.928C19.9484 20.928..." fill="black"/> <!-- Left eye -->
<path d="M30.0729 20.928C28.093 20.928..." fill="black"/> <!-- Right eye -->
```

## Animations Preserved
- `gentleFloat`: 3s ease-in-out infinite (ghost floating)
- `fadeIn`: 0.2s ease-out (window entrance)
- `slideIn`: 0.3s ease-out (message entrance)
- `typing`: 1.4s infinite (typing indicator dots)

## Testing Recommendations
1. Restart the Kiro assistant application
2. Verify the ghost icon appears correctly in all windows
3. Check that gradients render properly
4. Test dark/light theme compatibility
5. Verify hover states and animations work smoothly

## Next Steps (Optional)
- Update system tray icon to use the official Kiro icon
- Consider adding theme variants (light/dark) with appropriate color adjustments
- Add loading animations using the lineLoader.svg or logoLoader.svg assets
