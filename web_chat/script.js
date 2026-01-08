class ChatClient {
    constructor() {
        this.inputPort = 8080;
        this.outputPort = 8081;
        this.serverHost = 'localhost';
        this.isConnected = false;
        this.outputPollingInterval = null;
        this.eventSource = null;

        this.initializeElements();
        this.bindEvents();
        this.loadSettings();
        this.connect();
    }

    initializeElements() {
        // Chat elements
        this.chatMessages = document.getElementById('chatMessages');
        this.messageInput = document.getElementById('messageInput');
        this.sendButton = document.getElementById('sendButton');
        this.connectionDot = document.getElementById('connectionDot');

        // Settings elements
        this.settingsModal = document.getElementById('settingsModal');
        this.settingsBtn = document.getElementById('settingsBtn');
        this.closeSettings = document.getElementById('closeSettings');
        this.inputPortInput = document.getElementById('inputPort');
        this.outputPortInput = document.getElementById('outputPort');
        this.serverHostInput = document.getElementById('serverHost');
        this.saveSettingsBtn = document.getElementById('saveSettings');
        
        // Sidebar elements
        this.newChatBtn = document.getElementById('newChatBtn');
    }

    bindEvents() {
        // Message sending
        this.sendButton.addEventListener('click', () => this.sendMessage());
        this.messageInput.addEventListener('keydown', (e) => {
            if (e.key === 'Enter' && !e.shiftKey) {
                e.preventDefault();
                this.sendMessage();
            }
        });

        // Auto-resize textarea
        this.messageInput.addEventListener('input', () => {
            this.messageInput.style.height = 'auto';
            this.messageInput.style.height = (this.messageInput.scrollHeight) + 'px';
            if (this.messageInput.value === '') {
                this.messageInput.style.height = 'auto'; // Reset when empty
            }
        });

        // Settings modal
        this.settingsBtn.addEventListener('click', () => {
            this.settingsModal.classList.add('open');
        });

        this.closeSettings.addEventListener('click', () => {
            this.settingsModal.classList.remove('open');
        });

        this.saveSettingsBtn.addEventListener('click', () => {
            this.saveSettings();
            this.disconnect();
            setTimeout(() => this.connect(), 1000);
            this.settingsModal.classList.remove('open');
        });

        // Close settings when clicking outside
        this.settingsModal.addEventListener('click', (e) => {
            if (e.target === this.settingsModal) {
                this.settingsModal.classList.remove('open');
            }
        });
        
        // New Chat
        this.newChatBtn.addEventListener('click', () => {
            this.clearChat();
        });
    }

    loadSettings() {
        const settings = localStorage.getItem('chatSettings');
        if (settings) {
            const parsed = JSON.parse(settings);
            this.inputPort = parsed.inputPort || 8080;
            this.outputPort = parsed.outputPort || 8081;
            this.serverHost = parsed.serverHost || 'localhost';

            this.inputPortInput.value = this.inputPort;
            this.outputPortInput.value = this.outputPort;
            this.serverHostInput.value = this.serverHost;
        }
    }

    saveSettings() {
        this.inputPort = parseInt(this.inputPortInput.value) || 8080;
        this.outputPort = parseInt(this.outputPortInput.value) || 8081;
        this.serverHost = this.serverHostInput.value || 'localhost';

        const settings = {
            inputPort: this.inputPort,
            outputPort: this.outputPort,
            serverHost: this.serverHost
        };
        localStorage.setItem('chatSettings', JSON.stringify(settings));
    }

    updateConnectionStatus(status) {
        // Update the dot color
        this.connectionDot.className = 'connection-dot'; // Reset
        switch (status) {
            case 'connected':
                this.connectionDot.classList.add('connected');
                this.isConnected = true;
                break;
            case 'disconnected':
            case 'error':
                this.connectionDot.classList.add('disconnected');
                this.isConnected = false;
                break;
            case 'connecting':
                this.connectionDot.classList.add('connecting');
                this.isConnected = false;
                break;
        }
    }

    async connect() {
        this.updateConnectionStatus('connecting');

        try {
            // Test input server connection
            const inputUrl = `http://${this.serverHost}:${this.inputPort}/health`;
            // Note: We might want to use a short timeout for health check
            const controller = new AbortController();
            const timeoutId = setTimeout(() => controller.abort(), 2000);
            
            try {
                 const inputResponse = await fetch(inputUrl, { signal: controller.signal });
                 clearTimeout(timeoutId);
                 if (!inputResponse.ok) throw new Error('Input server not responding');
            } catch (e) {
                 clearTimeout(timeoutId);
                 throw e;
            }

            // Test output server connection
            const outputUrl = `http://${this.serverHost}:${this.outputPort}/health`;
             const controller2 = new AbortController();
            const timeoutId2 = setTimeout(() => controller2.abort(), 2000);
            
            try {
                 const outputResponse = await fetch(outputUrl, { signal: controller2.signal });
                 clearTimeout(timeoutId2);
                 if (!outputResponse.ok) throw new Error('Output server not responding');
            } catch (e) {
                 clearTimeout(timeoutId2);
                 throw e;
            }

            this.updateConnectionStatus('connected');
            this.startOutputPolling();

        } catch (error) {
            console.error('Connection failed:', error);
            this.updateConnectionStatus('error');
            // Retry connection after 5 seconds
            setTimeout(() => this.connect(), 5000);
        }
    }

    disconnect() {
        this.updateConnectionStatus('disconnected');
        if (this.eventSource) {
            this.eventSource.close();
            this.eventSource = null;
        }
    }

    startOutputPolling() {
        if (this.eventSource) {
            this.eventSource.close();
        }

        const url = `http://${this.serverHost}:${this.outputPort}/api/subscribe`;
        this.eventSource = new EventSource(url);

        this.eventSource.onopen = () => {
            console.log('EventSource connection established');
            this.updateConnectionStatus('connected');
        };

        this.eventSource.onmessage = (event) => {
            try {
                const message = JSON.parse(event.data);
                this.displayBotMessage(message);
            } catch (error) {
                console.error('Failed to parse message:', error);
            }
        };

        this.eventSource.onerror = (error) => {
            console.error('EventSource error:', error);
            this.updateConnectionStatus('error');
            this.eventSource.close();
            this.eventSource = null;

            // Try to reconnect after 3 seconds
            if (this.isConnected) { // Only reconnect if we think we should be connected
                setTimeout(() => {
                    console.log('Attempting to reconnect to EventSource...');
                    this.startOutputPolling();
                }, 3000);
            }
        };
    }

    async sendMessage() {
        const content = this.messageInput.value.trim();
        if (!content || !this.isConnected) return;

        this.setInputEnabled(false);

        try {
            this.displayUserMessage(content);
            this.messageInput.value = '';
            this.messageInput.style.height = 'auto'; // Reset height

            const url = `http://${this.serverHost}:${this.inputPort}/api/send`;
            const response = await fetch(url, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    content: content,
                    timestamp: Date.now()
                })
            });

            if (!response.ok) {
                throw new Error('Failed to send message');
            }
            
            // We don't display result here, we wait for SSE

        } catch (error) {
            console.error('Send message failed:', error);
            this.displaySystemMessage(`Send failed: ${error.message}`);
        } finally {
            this.setInputEnabled(true);
            this.messageInput.focus();
        }
    }

    setInputEnabled(enabled) {
        this.messageInput.disabled = !enabled;
        this.sendButton.disabled = !enabled;
    }
    
    extractTextFromContent(data) {
        if (data == null) return '';
        if (typeof data === 'string') return data;
        if (Array.isArray(data)) {
            return data.map(d => this.extractTextFromContent(d)).filter(s => s && s.trim() !== '').join('\n\n');
        }
        if (typeof data === 'object') {
            if (typeof data.text === 'string') return data.text;
            if (typeof data.content === 'string') return data.content;
            if (Array.isArray(data.content)) return this.extractTextFromContent(data.content);
            if (data.message) return this.extractTextFromContent(data.message);
            const parts = [];
            for (const k of Object.keys(data)) {
                const t = this.extractTextFromContent(data[k]);
                if (t && t.trim() !== '') parts.push(t);
            }
            return parts.join('\n\n');
        }
        return '';
    }

    createMessageElement(content, isUser) {
        const messageDiv = document.createElement('div');
        messageDiv.className = `message ${isUser ? 'user-message' : 'bot-message'}`;

        const innerDiv = document.createElement('div');
        innerDiv.className = 'message-inner';

        // Avatar
        const avatarDiv = document.createElement('div');
        avatarDiv.className = 'message-avatar';
        if (isUser) {
             // User Icon
             avatarDiv.innerHTML = `<svg stroke="currentColor" fill="none" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round" height="1.5em" width="1.5em" xmlns="http://www.w3.org/2000/svg"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"></path><circle cx="12" cy="7" r="4"></circle></svg>`;
        } else {
             // Bot Icon (Green)
             avatarDiv.innerHTML = `<svg stroke="currentColor" fill="none" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round" height="1.5em" width="1.5em" xmlns="http://www.w3.org/2000/svg"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect><line x1="12" y1="8" x2="12" y2="16"></line><line x1="8" y1="12" x2="16" y2="12"></line></svg>`; // Simple placeholder for now, maybe the robot icon
        }

        // Content
        const contentDiv = document.createElement('div');
        contentDiv.className = 'message-content';
        
        if (!isUser && typeof content !== 'string') {
             contentDiv.innerHTML = `<pre><code>${JSON.stringify(content, null, 2)}</code></pre>`;
        } else {
             if (!isUser && typeof content === 'string' && window.marked && window.DOMPurify) {
                 const html = window.marked.parse(content || '');
                 contentDiv.innerHTML = window.DOMPurify.sanitize(html);
             } else {
                 contentDiv.textContent = content;
             }
        }

        innerDiv.appendChild(avatarDiv);
        innerDiv.appendChild(contentDiv);
        messageDiv.appendChild(innerDiv);

        return messageDiv;
    }

    displayUserMessage(content) {
        const msgEl = this.createMessageElement(content, true);
        this.chatMessages.appendChild(msgEl);
        this.scrollToBottom();
    }

    displayBotMessage(messageData) {
        const contentText = this.extractTextFromContent(messageData?.content ?? messageData);
        const content = contentText && contentText.trim() !== '' ? contentText : JSON.stringify(messageData, null, 2);

        const msgEl = this.createMessageElement(content, false);
        this.chatMessages.appendChild(msgEl);
        this.scrollToBottom();
    }
    
    displaySystemMessage(text) {
         const div = document.createElement('div');
         div.className = 'message system-message';
         div.innerHTML = `<div class="message-inner"><div class="message-content" style="color: #ef4444">${text}</div></div>`;
         this.chatMessages.appendChild(div);
         this.scrollToBottom();
    }

    scrollToBottom() {
        this.chatMessages.scrollTop = this.chatMessages.scrollHeight;
        // Also scroll main content if needed, though chat-messages has overflow-y: auto
    }
    
    clearChat() {
        this.chatMessages.innerHTML = `
            <div class="message system-message">
                <div class="message-inner">
                    <div class="message-content">
                        Robot Chat
                    </div>
                </div>
            </div>
        `;
    }
}

// Initialize
document.addEventListener('DOMContentLoaded', () => {
    window.chatClient = new ChatClient();
});
