// Markdown rendering with code block and mermaid support

export function initMarkdown() {
    mermaid.initialize({ 
        startOnLoad: false,
        theme: 'default',
        securityLevel: 'loose',
        flowchart: {
            useMaxWidth: true,
            htmlLabels: true,
            curve: 'basis'
        }
    });
}

export function renderMarkdown(markdown, targetElement) {
    if (!markdown) {
        targetElement.innerHTML = '';
        return;
    }
    
    marked.setOptions({
        breaks: true,
        gfm: true
    });
    
    const html = marked.parse(markdown);
    targetElement.innerHTML = html;
    
    const codeBlocks = targetElement.querySelectorAll('pre code');
    codeBlocks.forEach((codeBlock) => {
        const pre = codeBlock.parentElement;
        const className = codeBlock.className;
        const langMatch = className.match(/language-(\w+)/);
        const language = langMatch ? langMatch[1] : 'text';
        
        if (language === 'mermaid') {
            renderMermaidDiagram(codeBlock, pre);
            return;
        }
        
        if (language && language !== 'text' && Prism.languages[language]) {
            try {
                const code = codeBlock.textContent;
                const highlighted = Prism.highlight(code, Prism.languages[language], language);
                codeBlock.innerHTML = highlighted;
                codeBlock.className = `language-${language}`;
            } catch (e) {
                console.warn(`Failed to highlight ${language}:`, e);
            }
        }
        
        wrapCodeBlock(codeBlock, pre, language);
    });
}

function wrapCodeBlock(codeBlock, pre, language) {
    const wrapper = document.createElement('div');
    wrapper.className = 'code-block-wrapper';
    
    const header = document.createElement('div');
    header.className = 'code-block-header';
    
    const langLabel = document.createElement('span');
    langLabel.className = 'code-block-language';
    langLabel.textContent = language;
    
    const copyBtn = createCopyButton(codeBlock.textContent);
    
    header.appendChild(langLabel);
    header.appendChild(copyBtn);
    
    pre.parentNode.insertBefore(wrapper, pre);
    wrapper.appendChild(header);
    wrapper.appendChild(pre);
}

async function renderMermaidDiagram(codeBlock, pre) {
    const code = codeBlock.textContent;
    
    const wrapper = document.createElement('div');
    wrapper.className = 'mermaid-wrapper';
    
    const header = document.createElement('div');
    header.className = 'mermaid-header';
    
    const label = document.createElement('span');
    label.className = 'mermaid-label';
    label.textContent = 'Diagram';
    
    const copyBtn = createCopyButton(code);
    
    header.appendChild(label);
    header.appendChild(copyBtn);
    
    const mermaidDiv = document.createElement('div');
    mermaidDiv.className = 'mermaid';
    mermaidDiv.textContent = code;
    
    pre.parentNode.insertBefore(wrapper, pre);
    wrapper.appendChild(header);
    wrapper.appendChild(mermaidDiv);
    pre.remove();
    
    try {
        await mermaid.run({ nodes: [mermaidDiv] });
    } catch (error) {
        console.error('Mermaid rendering error:', error);
        mermaidDiv.innerHTML = `<div style="color: #dc2626; padding: 20px;">Error rendering diagram: ${error.message}</div>`;
    }
}

function createCopyButton(code) {
    const copyBtn = document.createElement('button');
    copyBtn.className = 'copy-button';
    copyBtn.innerHTML = `
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
            <rect x="9" y="9" width="13" height="13" rx="2" ry="2"></rect>
            <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1"></path>
        </svg>
        <span>Copy</span>
    `;
    copyBtn.onclick = () => copyCode(code, copyBtn);
    return copyBtn;
}

function copyCode(code, button) {
    navigator.clipboard.writeText(code).then(() => {
        const originalHTML = button.innerHTML;
        button.classList.add('copied');
        button.innerHTML = `
            <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
                <polyline points="20 6 9 17 4 12"></polyline>
            </svg>
            <span>Copied!</span>
        `;
        
        setTimeout(() => {
            button.classList.remove('copied');
            button.innerHTML = originalHTML;
        }, 2000);
    }).catch(err => {
        console.error('Failed to copy code:', err);
    });
}
