class ChatClient {
    constructor() {
        this.inputPort = 8080;
        this.outputPort = 8081;
        this.serverHost = 'localhost';
        this.isConnected = false;
        this.outputPollingInterval = null;
        this.eventSource = null;

        this.sessionId = null;
        this.isBroadcastMode = false;
        this.sessions = [];
        this.sessionMessages = {};
        this.activeProgressBars = {};

        this.initializeElements();
        this.bindEvents();
        this.loadSettings();
        this.loadState();
        this.connect();
    }

    initializeElements() {
        // Chat elements
        this.chatMessages = document.getElementById('chatMessages');
        this.messageInput = document.getElementById('messageInput');
        this.sendButton = document.getElementById('sendButton');
        this.connectionDot = document.getElementById('connectionDot');
        
        // File upload elements
        this.fileInput = document.getElementById('fileInput');
        this.attachBtn = document.getElementById('attachBtn');
        this.filePreview = document.getElementById('filePreview');
        this.selectedFiles = [];

        // Settings elements
        this.settingsModal = document.getElementById('settingsModal');
        this.settingsBtn = document.getElementById('settingsBtn');
        this.closeSettings = document.getElementById('closeSettings');
        this.inputPortInput = document.getElementById('inputPort');
        this.outputPortInput = document.getElementById('outputPort');
        this.serverHostInput = document.getElementById('serverHost');
        this.saveSettingsBtn = document.getElementById('saveSettings');
        this.broadcastModeInput = document.getElementById('broadcastMode');
        this.sessionIdDisplay = document.getElementById('sessionIdDisplay');
        
        // Sidebar elements
        this.newChatBtn = document.getElementById('newChatBtn');
        this.historyList = document.getElementById('historyList');
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
        
        // File attachment
        this.attachBtn.addEventListener('click', () => this.fileInput.click());
        this.fileInput.addEventListener('change', (e) => this.handleFileSelect(e));

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
            this.createNewChat();
        });
    }
    
    handleFileSelect(e) {
        if (e.target.files && e.target.files.length > 0) {
            const files = Array.from(e.target.files);
            this.selectedFiles = [...this.selectedFiles, ...files];
            this.renderFilePreview();
            this.fileInput.value = '';
        }
    }

    renderFilePreview() {
        this.filePreview.innerHTML = '';
        this.selectedFiles.forEach((file, index) => {
            const chip = document.createElement('div');
            chip.className = 'file-chip';
            
            let progressHtml = '';
            if (file.uploadProgress !== undefined) {
                progressHtml = `<span class="upload-progress" style="margin-left:5px; color:#10a37f; font-size:0.7em;">${file.uploadProgress}%</span>`;
            }

            chip.innerHTML = `
                <span>${file.name}</span>
                ${progressHtml}
                <span class="remove-file" data-index="${index}">×</span>
            `;
            chip.querySelector('.remove-file').addEventListener('click', (e) => {
                e.stopPropagation();
                this.selectedFiles.splice(index, 1);
                this.renderFilePreview();
            });
            this.filePreview.appendChild(chip);
        });
    }

    async calculateMD5(file) {
        return new Promise((resolve, reject) => {
            const blobSlice = File.prototype.slice || File.prototype.mozSlice || File.prototype.webkitSlice;
            const chunkSize = 2097152; // 2MB
            const chunks = Math.ceil(file.size / chunkSize);
            let currentChunk = 0;
            const spark = new SparkMD5.ArrayBuffer();
            const fileReader = new FileReader();

            fileReader.onload = function(e) {
                spark.append(e.target.result);
                currentChunk++;
                if (currentChunk < chunks) {
                    loadNext();
                } else {
                    resolve(spark.end());
                }
            };

            fileReader.onerror = function() {
                reject('MD5 calculation failed');
            };

            function loadNext() {
                const start = currentChunk * chunkSize;
                const end = ((start + chunkSize) >= file.size) ? file.size : start + chunkSize;
                fileReader.readAsArrayBuffer(blobSlice.call(file, start, end));
            }

            loadNext();
        });
    }

    async uploadFileWithProgress(file, onProgress) {
        // 1. Calc MD5
        onProgress(0); // Start
        const md5 = await this.calculateMD5(file);
        
        // 2. Check Exists
        const checkRes = await fetch(`http://${this.serverHost}:${this.inputPort}/api/check_file`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ md5, filename: file.name })
        });
        const checkData = await checkRes.json();
        if (checkData.exists) {
            onProgress(100);
            return checkData.file.path;
        }

        // 3. Upload
        return new Promise((resolve, reject) => {
            const xhr = new XMLHttpRequest();
            const formData = new FormData();
            formData.append('files', file);

            xhr.upload.addEventListener('progress', (e) => {
                if (e.lengthComputable) {
                    const percent = Math.round((e.loaded / e.total) * 100);
                    onProgress(percent);
                }
            });

            xhr.addEventListener('load', () => {
                if (xhr.status >= 200 && xhr.status < 300) {
                    const resp = JSON.parse(xhr.responseText);
                    resolve(resp.data.files[0]);
                } else {
                    reject(new Error('Upload failed'));
                }
            });

            xhr.addEventListener('error', () => reject(new Error('Network error')));
            
            xhr.open('POST', `http://${this.serverHost}:${this.inputPort}/api/upload`);
            xhr.send(formData);
        });
    }

    async uploadFiles() {
        if (this.selectedFiles.length === 0) return [];
        
        const uploadedPaths = [];
        
        for (let i = 0; i < this.selectedFiles.length; i++) {
            const file = this.selectedFiles[i];
            
            const onProgress = (percent) => {
                file.uploadProgress = percent;
                this.renderFilePreview();
            };

            try {
                const path = await this.uploadFileWithProgress(file, onProgress);
                uploadedPaths.push(path);
            } catch (e) {
                console.error(`Upload failed for ${file.name}:`, e);
                alert(`Upload failed for ${file.name}`);
                throw e;
            }
        }
        return uploadedPaths;
    }

    loadState() {
         const state = localStorage.getItem('chatState');
         if (state) {
             const parsed = JSON.parse(state);
             this.sessions = parsed.sessions || [];
             this.sessionMessages = parsed.sessionMessages || {};
             this.sessionId = parsed.currentSessionId;
         }
    }

    saveState() {
        const state = {
            sessions: this.sessions,
            sessionMessages: this.sessionMessages,
            currentSessionId: this.sessionId
        };
        localStorage.setItem('chatState', JSON.stringify(state));
    }

    async createNewChat() {
        try {
            if (this.sessions.length >= 10) {
                const removed = this.sessions.pop();
                if (removed) delete this.sessionMessages[removed.id];
            }

            const sessionUrl = `http://${this.serverHost}:${this.inputPort}/api/session`;
            const res = await fetch(sessionUrl, { method: 'POST' });
            if (!res.ok) throw new Error('Failed to create session');
            const data = await res.json();
            const newId = data.session_id;

            const newSession = {
                id: newId,
                title: `Chat ${new Date().toLocaleTimeString()}`, // Better naming
                created_at: Date.now()
            };
            this.sessions.unshift(newSession);
            this.sessionMessages[newId] = [];
            
            this.switchSession(newId);
        } catch (e) {
            console.error("Failed to create new chat:", e);
        }
    }

    deleteSession(id, event) {
        if (event) event.stopPropagation();
        
        const index = this.sessions.findIndex(s => s.id === id);
        if (index === -1) return;

        this.sessions.splice(index, 1);
        delete this.sessionMessages[id];

        if (this.sessionId === id) {
            if (this.sessions.length > 0) {
                this.switchSession(this.sessions[0].id);
            } else {
                this.createNewChat();
            }
        } else {
            this.renderHistoryList();
            this.saveState();
        }
    }

    switchSession(id, force = false) {
        if (!force && this.sessionId === id) return;

        this.sessionId = id;
        if (this.sessionIdDisplay) this.sessionIdDisplay.textContent = id;
        
        this.renderHistoryList();

        // Clear and restore
        this.activeProgressBars = {};
        this.chatMessages.innerHTML = '';
        const messages = this.sessionMessages[id] || [];
        messages.forEach(msg => {
            if (msg.type === 'user') {
                 if (msg.isOther) this.displayOtherUserMessage(msg.content, msg.files, false);
                 else this.displayUserMessage(msg.content, msg.files, false);
            } else {
                 this.displayBotMessage(msg.content, false);
            }
        });

        // Reconnect
        this.startOutputPolling();
        this.saveState();
    }

    renderHistoryList() {
        this.historyList.innerHTML = '';
        this.sessions.forEach(session => {
            const btn = document.createElement('div');
            btn.className = `nav-item ${session.id === this.sessionId ? 'active' : ''}`;
            
            const content = document.createElement('div');
            content.className = 'nav-item-content';
            content.innerHTML = `
                <svg stroke="currentColor" fill="none" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round" height="1em" width="1em" xmlns="http://www.w3.org/2000/svg"><path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z"></path></svg>
                <span class="text-truncate">${session.title}</span>
            `;
            content.addEventListener('click', () => this.switchSession(session.id));

            const delBtn = document.createElement('button');
            delBtn.className = 'delete-chat-btn';
            delBtn.innerHTML = '<svg stroke="currentColor" fill="none" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round" height="1em" width="1em" xmlns="http://www.w3.org/2000/svg"><line x1="18" y1="6" x2="6" y2="18"></line><line x1="6" y1="6" x2="18" y2="18"></line></svg>';
            delBtn.title = 'Delete chat';
            delBtn.addEventListener('click', (e) => this.deleteSession(session.id, e));

            btn.appendChild(content);
            btn.appendChild(delBtn);
            this.historyList.appendChild(btn);
        });
    }

    loadSettings() {
        const settings = localStorage.getItem('chatSettings');
        if (settings) {
            const parsed = JSON.parse(settings);
            this.inputPort = parsed.inputPort || 8080;
            this.outputPort = parsed.outputPort || 8081;
            this.serverHost = parsed.serverHost || 'localhost';
            this.isBroadcastMode = parsed.isBroadcastMode || false;
            // if (parsed.sessionId) {
            //     this.sessionId = parsed.sessionId;
            // }
        }
        
        // Update UI
        this.inputPortInput.value = this.inputPort;
        this.outputPortInput.value = this.outputPort;
        this.serverHostInput.value = this.serverHost;
        this.broadcastModeInput.checked = this.isBroadcastMode;
        if (this.sessionId) {
            this.sessionIdDisplay.textContent = this.sessionId;
        }
    }

    saveSettings() {
        this.inputPort = parseInt(this.inputPortInput.value) || 8080;
        this.outputPort = parseInt(this.outputPortInput.value) || 8081;
        this.serverHost = this.serverHostInput.value || 'localhost';
        this.isBroadcastMode = this.broadcastModeInput.checked;

        const settings = {
            inputPort: this.inputPort,
            outputPort: this.outputPort,
            serverHost: this.serverHost,
            isBroadcastMode: this.isBroadcastMode,
            // sessionId: this.sessionId // Don't save session ID to ensure fresh one on reload
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

            // Create new session from backend
            let needNewSession = !this.sessionId;
            if (this.sessionId && !this.sessions.some(s => s.id === this.sessionId)) {
                needNewSession = true;
            }

            if (needNewSession) {
                const sessionUrl = `http://${this.serverHost}:${this.inputPort}/api/session`;
                const sessionRes = await fetch(sessionUrl, { method: 'POST' });
                if (!sessionRes.ok) throw new Error('Failed to create session');
                const sessionData = await sessionRes.json();
                const newId = sessionData.session_id;
                
                // Add initial session to list
                const newSession = {
                    id: newId,
                    title: `Chat ${this.sessions.length + 1}`,
                    created_at: Date.now()
                };
                this.sessions.unshift(newSession);
                this.sessionMessages[newId] = [];
                this.switchSession(newId, true);
            } else {
                this.switchSession(this.sessionId, true);
            }

            if (this.sessionIdDisplay) {
                this.sessionIdDisplay.textContent = this.sessionId;
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

        const url = `http://${this.serverHost}:${this.outputPort}/api/subscribe?session_id=${this.sessionId}`;
        this.eventSource = new EventSource(url);

        this.eventSource.onopen = () => {
            console.log('EventSource connection established');
            this.updateConnectionStatus('connected');
        };

        this.eventSource.onmessage = (event) => {
            try {
                const message = JSON.parse(event.data);
                this.handleIncomingMessage(message);
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

    handleIncomingMessage(message) {
        if (message.session_id && message.session_id !== this.sessionId && !this.isBroadcastMode) {
            return;
        }

        const isMine = message.session_id === this.sessionId;
        const isUserSource = message.source === 'user';
        
        if (isUserSource) {
            // It's a user message (echo)
            let content = message.content;
            let files = [];
            
            if (typeof content === 'object') {
                if (content.files) files = content.files;
                if (content.content) content = content.content;
            }

            if (isMine) {
                this.displayUserMessage(content, files);
            } else {
                this.displayOtherUserMessage(content, files);
            }
        } else {
            // System/Bot message
            if (message.content && message.content.type === 'progress') {
                this.updateProgress(message.content);
            } else {
                this.displayBotMessage(message);
            }
        }
    }

    updateProgress(data) {
        console.log('updateProgress called with:', data);
        // data: { message, progress, total, token, type: 'progress' }
        const { token, progress, total, message } = data;
        
        const p = parseFloat(progress);
        const t = parseFloat(total);
        
        let percent = (t > 0) ? (p / t) * 100 : 0;
        if (percent > 100) percent = 100;
        if (percent < 0) percent = 0;

        // Ensure token is handled consistently as a string key
        const tokenKey = (token !== undefined && token !== null) ? String(token) : 'default';

        let progressEl = this.activeProgressBars[tokenKey];

        if (!progressEl) {
            console.log('Creating new progress bar for token:', tokenKey);
            // Create new progress message
            const msgDiv = document.createElement('div');
            msgDiv.className = 'message bot-message progress-msg-container';
            
            const innerDiv = document.createElement('div');
            innerDiv.className = 'message-inner';

            // Avatar (Bot)
            const avatarDiv = document.createElement('div');
            avatarDiv.className = 'message-avatar';
            avatarDiv.innerHTML = `<svg stroke="currentColor" fill="none" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round" height="1.5em" width="1.5em" xmlns="http://www.w3.org/2000/svg"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect><line x1="12" y1="8" x2="12" y2="16"></line><line x1="8" y1="12" x2="16" y2="12"></line></svg>`;

            const contentDiv = document.createElement('div');
            contentDiv.className = 'message-content progress-message';
            
            // Initialize with 0 value
            contentDiv.innerHTML = `
                <div class="progress-info">${message || 'Processing...'}</div>
                <div class="progress-wrapper" style="position: relative; width: 100%;">
                    <progress class="mcp-progress" value="0" max="100"></progress>
                    <div class="progress-text">${Math.round(percent)}%</div>
                </div>
            `;

            innerDiv.appendChild(avatarDiv);
            innerDiv.appendChild(contentDiv);
            msgDiv.appendChild(innerDiv);
            
            this.chatMessages.appendChild(msgDiv);
            this.scrollToBottom();

            progressEl = {
                container: msgDiv,
                bar: contentDiv.querySelector('progress.mcp-progress'),
                text: contentDiv.querySelector('.progress-text'),
                info: contentDiv.querySelector('.progress-info')
            };
            this.activeProgressBars[tokenKey] = progressEl;

            // Trigger animation after a slight delay
            setTimeout(() => {
                if (progressEl && progressEl.bar) {
                    progressEl.bar.value = percent;
                    console.log('Initial value set to:', percent);
                }
            }, 50);

        } else {
            console.log('Updating existing progress bar for token:', tokenKey, 'to', percent + '%');
            // Update existing
            progressEl.bar.value = percent;
            progressEl.text.textContent = `${Math.round(percent)}%`;
            if (message) progressEl.info.textContent = message;
        }

        if (p >= t) {
            console.log('Progress complete for token:', tokenKey);
            // Optional: Mark as complete or remove from active map after some time
             setTimeout(() => {
                delete this.activeProgressBars[tokenKey];
             }, 1000);
        }
    }

    async sendMessage() {
        const content = this.messageInput.value.trim();
        const hasFiles = this.selectedFiles.length > 0;
        
        if ((!content && !hasFiles) || !this.isConnected) return;

        this.setInputEnabled(false);

        try {
            let uploadedFiles = [];
            if (hasFiles) {
                uploadedFiles = await this.uploadFiles();
            }

            this.messageInput.value = '';
            this.messageInput.style.height = 'auto';
            
            this.selectedFiles = [];
            this.renderFilePreview();

            const url = `http://${this.serverHost}:${this.inputPort}/api/send/${this.sessionId}`;
            const response = await fetch(url, {
                method: 'POST',
                headers: {
                    'Content-Type': 'application/json',
                },
                body: JSON.stringify({
                    content: content,
                    timestamp: Date.now(),
                    session_id: this.sessionId,
                    files: uploadedFiles.length > 0 ? uploadedFiles : undefined
                })
            });

            if (!response.ok) {
                throw new Error('Failed to send message');
            }
            
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
        this.attachBtn.disabled = !enabled;
        this.fileInput.disabled = !enabled;
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
                if (k === 'type' && data[k] === 'user_message') continue;
                if (k === 'timestamp') continue;
                if (k === 'files') continue;
                const t = this.extractTextFromContent(data[k]);
                if (t && t.trim() !== '') parts.push(t);
            }
            return parts.join('\n\n');
        }
        return '';
    }

    createMessageElement(content, isUser, isOtherUser = false, files = []) {
        const messageDiv = document.createElement('div');
        let className = 'message';
        if (isUser) className += ' user-message';
        else if (isOtherUser) className += ' other-user-message';
        else className += ' bot-message';
        messageDiv.className = className;

        const innerDiv = document.createElement('div');
        innerDiv.className = 'message-inner';

        // Avatar
        const avatarDiv = document.createElement('div');
        avatarDiv.className = 'message-avatar';
        if (isUser || isOtherUser) {
             // User Icon
             avatarDiv.innerHTML = `<svg stroke="currentColor" fill="none" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round" height="1.5em" width="1.5em" xmlns="http://www.w3.org/2000/svg"><path d="M20 21v-2a4 4 0 0 0-4-4H8a4 4 0 0 0-4 4v2"></path><circle cx="12" cy="7" r="4"></circle></svg>`;
        } else {
             // Bot Icon (Green)
             avatarDiv.innerHTML = `<svg stroke="currentColor" fill="none" stroke-width="2" viewBox="0 0 24 24" stroke-linecap="round" stroke-linejoin="round" height="1.5em" width="1.5em" xmlns="http://www.w3.org/2000/svg"><rect x="3" y="3" width="18" height="18" rx="2" ry="2"></rect><line x1="12" y1="8" x2="12" y2="16"></line><line x1="8" y1="12" x2="16" y2="12"></line></svg>`; 
        }

        // Content
        const contentDiv = document.createElement('div');
        contentDiv.className = 'message-content';
        
        if (!isUser && !isOtherUser && typeof content !== 'string') {
             contentDiv.innerHTML = `<pre><code>${JSON.stringify(content, null, 2)}</code></pre>`;
        } else {
             if (typeof content === 'string' && window.marked && window.DOMPurify) {
                 const html = window.marked.parse(content || '');
                 contentDiv.innerHTML = window.DOMPurify.sanitize(html);
             } else {
                 contentDiv.textContent = content;
             }
        }
        
        // Files
        if (files && files.length > 0) {
            const filesDiv = document.createElement('div');
            filesDiv.className = 'message-files';
            files.forEach(file => {
                 const name = file.split('/').pop().replace(/^[0-9a-f-]+_/, ''); // Try to clean uuid?
                 const fileEl = document.createElement('div');
                 fileEl.textContent = `📎 ${name}`;
                 fileEl.className = 'file-attachment';
                 filesDiv.appendChild(fileEl);
            });
            contentDiv.appendChild(filesDiv);
        }

        innerDiv.appendChild(avatarDiv);
        innerDiv.appendChild(contentDiv);
        messageDiv.appendChild(innerDiv);

        return messageDiv;
    }

    displayUserMessage(content, files = [], save = true) {
        if (save) {
            if (!this.sessionMessages[this.sessionId]) this.sessionMessages[this.sessionId] = [];
            this.sessionMessages[this.sessionId].push({
                type: 'user', isOther: false, content, files, timestamp: Date.now()
            });
            this.saveState();
        }

        const msgEl = this.createMessageElement(content, true, false, files);
        this.chatMessages.appendChild(msgEl);
        this.scrollToBottom();
    }
    
    displayOtherUserMessage(content, files = [], save = true) {
        if (save) {
            if (!this.sessionMessages[this.sessionId]) this.sessionMessages[this.sessionId] = [];
            this.sessionMessages[this.sessionId].push({
                type: 'user', isOther: true, content, files, timestamp: Date.now()
            });
            this.saveState();
        }

        const msgEl = this.createMessageElement(content, false, true, files);
        this.chatMessages.appendChild(msgEl);
        this.scrollToBottom();
    }

    displayBotMessage(messageData, save = true) {
        const contentText = this.extractTextFromContent(messageData?.content ?? messageData);
        const content = contentText && contentText.trim() !== '' ? contentText : JSON.stringify(messageData, null, 2);

        if (save) {
            if (!this.sessionMessages[this.sessionId]) this.sessionMessages[this.sessionId] = [];
            this.sessionMessages[this.sessionId].push({
                type: 'bot', content: content, timestamp: Date.now()
            });
            this.saveState();
        }

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
    }
    
    clearChat() {
        this.activeProgressBars = {};
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

document.addEventListener('DOMContentLoaded', () => {
    window.chatClient = new ChatClient();
});
