/**
 * Permission Modal Handler
 * Handles tool permission requests from the ACP
 */

const { invoke } = window.__TAURI__.tauri;
const { appWindow } = window.__TAURI__.window;

let currentPermissionRequest = null;

/**
 * Show permission modal
 */
async function showPermissionModal(notification) {
    const modal = document.getElementById('permissionModal');
    const toolTitle = document.getElementById('permissionToolTitle');
    
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
        options: params.options || []
    };
    
    // Update modal content
    toolTitle.textContent = title;
    
    // Show modal
    modal.style.display = 'flex';
    
    // Prevent window from hiding while modal is open
    try {
        await appWindow.setFocus();
        await appWindow.setAlwaysOnTop(true);
    } catch (error) {
        console.error('Failed to set window focus:', error);
    }
    
    // Resize window to fit modal (modal needs ~400px height)
    try {
        const currentSize = await appWindow.innerSize();
        const modalHeight = 400;
        const modalWidth = 520;
        
        // Only resize if current window is smaller than modal
        const newWidth = Math.max(currentSize.width, modalWidth);
        const newHeight = Math.max(currentSize.height, modalHeight);
        
        if (newWidth !== currentSize.width || newHeight !== currentSize.height) {
            await invoke('resize_floating_window', {
                width: newWidth,
                height: newHeight
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
    
    // Optionally resize window back to original size
    // For now, we'll leave it at the expanded size for better UX
}

/**
 * Handle permission response
 */
async function handlePermissionResponse(optionId) {
    if (!currentPermissionRequest) {
        console.error('No active permission request');
        return;
    }
    
    console.log('Handling permission response:', optionId);
    
    try {
        console.log('Sending permission response to backend...');
        await invoke('send_permission_response', {
            requestId: currentPermissionRequest.id,
            optionId: optionId,
            toolTitle: currentPermissionRequest.toolCall.title || 'Unknown'
        });
        
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
    const denyBtn = document.getElementById('permissionDeny');
    const onceBtn = document.getElementById('permissionOnce');
    const alwaysBtn = document.getElementById('permissionAlways');
    
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
    appWindow.listen('permission_request', (event) => {
        console.log('Permission request received:', event.payload);
        
        const { notification, auto_approve } = event.payload;
        
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
            // Show modal for user decision
            showPermissionModal(notification);
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
