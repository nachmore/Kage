/**
 * Permission Modal Handler
 * Handles tool permission requests from the ACP
 */

const { invoke } = window.__TAURI__.core;
const appWindow = window.__TAURI__.webviewWindow.getCurrentWebviewWindow();

let currentPermissionRequest = null;

/**
 * Get emoji for a tool name
 */
function getToolEmoji(name) {
    const lower = (name || '').toLowerCase();
    if (lower.includes('search')) return '🔍';
    if (lower.includes('fetch') || lower.includes('web')) return '🌐';
    if (lower.includes('read')) return '📖';
    if (lower.includes('write') || lower.includes('edit')) return '✏️';
    if (lower.includes('shell') || lower.includes('command') || lower.includes('terminal')) return '💻';
    if (lower.includes('aws') || lower.includes('cloud')) return '☁️';
    if (lower.includes('file')) return '📁';
    return '🔧';
}

function escapeHtml(text) {
    const div = document.createElement('div');
    div.textContent = text;
    return div.innerHTML;
}

/**
 * Show permission modal
 */
async function showPermissionModal(notification, toolName) {
    const modal = document.getElementById('permissionModal');
    const toolTitle = document.getElementById('permissionToolTitle');
    const toolNameEl = document.getElementById('permissionToolName');
    
    if (!modal || !toolTitle) {
        console.error('Permission modal elements not found');
        return;
    }
    
    // Extract tool information
    const params = notification.params || {};
    const toolCall = params.toolCall || {};
    const title = toolCall.title || 'Unknown Tool';
    
    // Store the current request
    currentPermissionRequest = {
        id: notification.id,
        sessionId: params.sessionId,
        toolCall: toolCall,
        options: params.options || [],
        toolName: toolName || null
    };
    
    // Update modal content
    toolTitle.textContent = title;
    
    // Show tool name with emoji if available
    if (toolNameEl) {
        if (toolName) {
            const emoji = getToolEmoji(toolName);
            toolNameEl.innerHTML = `<span class="tool-emoji">${emoji}</span><span class="tool-label">${escapeHtml(toolName)}</span>`;
            toolNameEl.style.display = 'flex';
        } else {
            toolNameEl.style.display = 'none';
        }
    }
    
    // Show modal
    modal.style.display = 'flex';
    
    // Prevent window from hiding while modal is open
    try {
        await appWindow.setFocus();
        await appWindow.setAlwaysOnTop(true);
    } catch (error) {
        console.error('Failed to set window focus:', error);
    }
    
    // Resize window to fit the permission modal.
    // Two-pass: first grow large enough, then measure and fine-tune.
    try {
        const scale = window.devicePixelRatio || 1;

        // First pass: grow to a generous size so the modal can lay out fully
        await invoke('resize_floating_window', {
            width: Math.round(540 * scale),
            height: Math.round(520 * scale)
        });

        // Wait for layout to settle
        await new Promise(resolve => setTimeout(resolve, 50));

        // Second pass: measure the actual modal and fit precisely
        const modalEl = modal.querySelector('.permission-modal');
        if (modalEl) {
            const rect = modalEl.getBoundingClientRect();
            // Account for overlay padding (20px top + 20px bottom) and some breathing room
            const neededHeight = Math.round((rect.height + 60) * scale);
            const neededWidth = Math.round(540 * scale);
            await invoke('resize_floating_window', {
                width: neededWidth,
                height: Math.max(neededHeight, Math.round(520 * scale))
            });
        }
    } catch (error) {
        console.error('Failed to resize window for modal:', error);
    }
}

/**
 * Hide permission modal
 */
async function hidePermissionModal() {
    const modal = document.getElementById('permissionModal');
    if (modal) {
        modal.style.display = 'none';
    }
    currentPermissionRequest = null;

    // Trigger a resize back to fit the current content
    try {
        // Use the FloatingApp's window manager if available
        if (window._floatingApp?.windowManager) {
            await window._floatingApp.windowManager.resizeWindow();
        }
    } catch (e) {
        // ignore
    }
}

/**
 * Handle permission response
 */
async function handlePermissionResponse(optionId, policyOverride) {
    if (!currentPermissionRequest) {
        console.error('No active permission request');
        return;
    }
    
    console.log('Handling permission response:', optionId, policyOverride || '');
    
    try {
        console.log('Sending permission response to backend...');
        // Use the resolved tool name for policy tracking, fall back to action title
        const policyTitle = currentPermissionRequest.toolName || currentPermissionRequest.toolCall.title || 'Unknown';
        await invoke('send_permission_response', {
            requestId: currentPermissionRequest.id,
            optionId: optionId,
            toolTitle: policyTitle
        });
        
        // If "Always Deny", update the tool policy to "deny"
        if (policyOverride) {
            await invoke('update_tool_policy', {
                toolTitle: policyTitle,
                policy: policyOverride
            });
        }
        
        console.log('Permission response sent successfully');
        
        // Small delay to ensure the response is processed
        await new Promise(resolve => setTimeout(resolve, 100));
        
        await hidePermissionModal();
        console.log('Modal hidden, conversation should continue');
    } catch (error) {
        console.error('Failed to send permission response:', error);
        alert('Failed to send permission response: ' + error);
    }
}

/**
 * Initialize permission modal
 */
function initPermissionModal() {
    // Set up button handlers
    const denyAlwaysBtn = document.getElementById('permissionDenyAlways');
    const denyBtn = document.getElementById('permissionDeny');
    const onceBtn = document.getElementById('permissionOnce');
    const alwaysBtn = document.getElementById('permissionAlways');
    
    if (denyAlwaysBtn) {
        denyAlwaysBtn.addEventListener('click', () => handlePermissionResponse('reject_once', 'deny'));
    }
    
    if (denyBtn) {
        denyBtn.addEventListener('click', () => handlePermissionResponse('reject_once'));
    }
    
    if (onceBtn) {
        onceBtn.addEventListener('click', () => handlePermissionResponse('allow_once'));
    }
    
    if (alwaysBtn) {
        alwaysBtn.addEventListener('click', () => handlePermissionResponse('allow_always'));
    }
    
    // Listen for permission requests from backend
    appWindow.listen('permission_request', async (event) => {
        console.log('Permission request received:', event.payload);
        
        const { notification, auto_approve } = event.payload;
        const requestSessionId = notification.params?.sessionId || '';

        // Only handle permission requests for the floating window's own session.
        // If the request is for a different session (e.g. one active in the main
        // chat window), ignore it here — the chat window will handle it.
        let floatingSessionId = null;
        try {
            floatingSessionId = await invoke('get_floating_session_id');
        } catch (e) { /* ignore */ }
        let currentSessionId = null;
        try {
            currentSessionId = await invoke('get_current_session_id');
        } catch (e) { /* ignore */ }

        // The floating window owns the "floating session". If the request is for
        // a session that isn't the current ACP session (which the floating window
        // would be using), skip it.
        const isFloatingSession = !requestSessionId
            || requestSessionId === floatingSessionId
            || requestSessionId === currentSessionId;

        // If the main chat window is visible and this isn't clearly the floating
        // session's request, let the chat window handle it.
        const isFloatingVisible = await appWindow.isVisible();
        if (!isFloatingVisible && !isFloatingSession) {
            console.log('Ignoring permission request for non-floating session:', requestSessionId);
            return;
        }
        
        if (auto_approve) {
            // Auto-approve the request
            console.log('Auto-approving permission request');
            invoke('send_permission_response', {
                requestId: notification.id,
                optionId: 'allow_once',
                toolTitle: notification.params?.toolCall?.title || 'Unknown'
            }).catch(error => {
                console.error('Failed to auto-approve:', error);
            });
        } else {
            // Double-check with the backend that this request is still pending.
            // It may have been auto-denied by dismiss_pending_permission already.
            let stillPending = false;
            try {
                stillPending = await invoke('has_pending_permission');
            } catch (e) { /* assume pending if check fails */ stillPending = true; }

            if (stillPending) {
                showPermissionModal(notification, event.payload.toolName);
            } else {
                console.log('Permission request already handled, skipping modal');
            }
        }
    });
    
    // Close modal on overlay click
    const modal = document.getElementById('permissionModal');
    if (modal) {
        modal.addEventListener('click', (e) => {
            if (e.target === modal) {
                handlePermissionResponse('reject_once');
            }
        });
    }

    // Esc to deny, block all keyboard input while modal is open
    document.addEventListener('keydown', (e) => {
        if (!currentPermissionRequest) return;
        if (e.key === 'Escape') {
            e.preventDefault();
            e.stopPropagation();
            handlePermissionResponse('reject_once');
        } else {
            // Block typing from reaching the input behind the modal
            e.preventDefault();
            e.stopPropagation();
        }
    }, true);
}

// Initialize when DOM is ready
if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', initPermissionModal);
} else {
    initPermissionModal();
}

// Export for use in other modules
window.PermissionModal = {
    show: showPermissionModal,
    hide: hidePermissionModal
};
