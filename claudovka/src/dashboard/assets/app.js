'use strict';

const state = {
  conversations: {},   // id → { conv, messages, element, liveDot }
  activeId: null,
  totalRequests: 0,
  ws: null,
  streamingId: null,
  streamingBubble: null,
};

// ─── Bootstrap ───────────────────────────────────────────────────────────────

async function init() {
  await loadConversations();
  connectWebSocket();

  document.getElementById('search').addEventListener('input', (e) => {
    filterConversations(e.target.value.toLowerCase());
  });
}

// ─── REST ────────────────────────────────────────────────────────────────────

async function loadConversations() {
  try {
    const res = await fetch('/api/conversations');
    const list = await res.json();
    for (const conv of list) {
      addConversation(conv, false);
    }
    updateCounter();
  } catch (e) {
    console.error('Failed to load conversations:', e);
  }
}

async function loadMessages(convId) {
  try {
    const res = await fetch(`/api/conversations/${convId}`);
    const data = await res.json();
    renderMessages(convId, data.messages || []);
  } catch (e) {
    console.error('Failed to load messages:', e);
  }
}

// ─── WebSocket ───────────────────────────────────────────────────────────────

function connectWebSocket() {
  const wsUrl = `ws://${location.host}/ws`;
  const ws = new WebSocket(wsUrl);
  state.ws = ws;

  ws.onopen = () => {
    document.getElementById('status-dot').className = '';
    console.log('WS connected');
  };

  ws.onclose = () => {
    document.getElementById('status-dot').className = 'error';
    setTimeout(connectWebSocket, 3000);
  };

  ws.onerror = () => {
    document.getElementById('status-dot').className = 'error';
  };

  ws.onmessage = (e) => {
    try {
      const msg = JSON.parse(e.data);
      handleWsMessage(msg);
    } catch (err) {
      console.error('Failed to parse WS message:', err);
    }
  };
}

function handleWsMessage(msg) {
  switch (msg.type) {
    case 'conversation_start':
      addConversation({
        id: msg.id,
        provider: msg.provider,
        model: msg.model,
        started_at: msg.timestamp,
        client_hint: null,
      }, true);
      state.totalRequests++;
      updateCounter();
      break;

    case 'message':
      appendMessage(msg.conversation_id, {
        role: msg.role || 'user',
        direction: msg.direction,
        content: msg.content,
        timestamp: msg.timestamp,
      });
      break;

    case 'text_delta':
      appendDelta(msg.conversation_id, msg.text);
      break;

    case 'response_complete':
      finalizeStream(msg.conversation_id, msg.tokens_in, msg.tokens_out);
      break;
  }
}

// ─── Conversation List ────────────────────────────────────────────────────────

function addConversation(conv, prepend) {
  if (state.conversations[conv.id]) return;

  const el = buildConvItem(conv);
  state.conversations[conv.id] = { conv, messages: [], element: el };

  const list = document.getElementById('conv-list');
  if (prepend) {
    list.prepend(el);
  } else {
    list.appendChild(el);
  }
}

function buildConvItem(conv) {
  const el = document.createElement('div');
  el.className = 'conv-item';
  el.dataset.id = conv.id;
  el.onclick = () => selectConversation(conv.id);

  const liveDot = document.createElement('div');
  liveDot.className = 'live-dot';

  const providerClass = `provider-${conv.provider || 'unknown'}`;
  const model = conv.model || 'unknown';
  const time = formatTime(conv.started_at);

  el.innerHTML = `
    <div class="conv-header">
      <span class="provider-badge ${providerClass}">${conv.provider || '?'}</span>
    </div>
    <div class="conv-model">${escHtml(model)}</div>
    <div class="conv-time">${time}</div>
  `;

  el.querySelector('.conv-header').appendChild(liveDot);

  // Store reference to live dot
  if (state.conversations[conv.id]) {
    state.conversations[conv.id].liveDot = liveDot;
  } else {
    // Will be set after insert
    setTimeout(() => {
      if (state.conversations[conv.id]) {
        state.conversations[conv.id].liveDot = liveDot;
      }
    }, 0);
  }

  return el;
}

function selectConversation(id) {
  // Deactivate previous
  if (state.activeId && state.conversations[state.activeId]) {
    state.conversations[state.activeId].element.classList.remove('active');
  }

  state.activeId = id;
  const entry = state.conversations[id];
  if (!entry) return;

  entry.element.classList.add('active');

  const { conv } = entry;
  document.getElementById('detail-header').querySelector('h2').textContent =
    `${conv.provider} / ${conv.model || 'unknown'}`;
  document.getElementById('detail-meta').textContent =
    `Started: ${formatTime(conv.started_at)}`;

  // Clear messages and load from server
  document.getElementById('messages').innerHTML = '';
  loadMessages(id);
}

function filterConversations(query) {
  const entries = Object.values(state.conversations);
  for (const { conv, element } of entries) {
    const text = `${conv.provider} ${conv.model} ${conv.started_at}`.toLowerCase();
    element.style.display = text.includes(query) ? '' : 'none';
  }
}

// ─── Messages ────────────────────────────────────────────────────────────────

function renderMessages(convId, messages) {
  const container = document.getElementById('messages');
  container.innerHTML = '';

  if (messages.length === 0) {
    container.innerHTML = '<div class="empty-state">No messages yet</div>';
    return;
  }

  for (const msg of messages) {
    const block = buildMessageBlock(msg.role, msg.content);
    container.appendChild(block);
  }
  container.scrollTop = container.scrollHeight;
}

function appendMessage(convId, msg) {
  if (convId !== state.activeId) return;
  const container = document.getElementById('messages');
  const block = buildMessageBlock(msg.role, msg.content);
  container.appendChild(block);
  container.scrollTop = container.scrollHeight;
}

function buildMessageBlock(role, content) {
  const block = document.createElement('div');
  const roleClass = role === 'user' ? 'user' : role === 'system' ? 'system' : 'assistant';
  block.className = `msg-block ${roleClass}`;

  const roleLabel = document.createElement('div');
  roleLabel.className = 'msg-role';
  roleLabel.textContent = role;

  const bubble = document.createElement('div');
  bubble.className = 'msg-bubble';

  if (role === 'system' && content.length > 500) {
    bubble.textContent = content.slice(0, 500) + '…';
    const toggle = document.createElement('button');
    toggle.className = 'collapsible-toggle';
    toggle.textContent = 'Show full system prompt';
    let expanded = false;
    toggle.onclick = () => {
      expanded = !expanded;
      bubble.textContent = expanded ? content : content.slice(0, 500) + '…';
      bubble.appendChild(toggle);
      toggle.textContent = expanded ? 'Collapse' : 'Show full system prompt';
    };
    bubble.appendChild(toggle);
  } else {
    bubble.textContent = content;
  }

  block.appendChild(roleLabel);
  block.appendChild(bubble);
  return block;
}

function appendDelta(convId, text) {
  if (convId !== state.activeId) return;

  if (state.streamingId !== convId) {
    // Start new streaming block
    const container = document.getElementById('messages');
    const block = buildMessageBlock('assistant', '');
    const cursor = document.createElement('span');
    cursor.className = 'cursor';
    block.querySelector('.msg-bubble').appendChild(cursor);
    container.appendChild(block);
    state.streamingId = convId;
    state.streamingBubble = block.querySelector('.msg-bubble');
  }

  if (state.streamingBubble) {
    const cursor = state.streamingBubble.querySelector('.cursor');
    const textNode = document.createTextNode(text);
    state.streamingBubble.insertBefore(textNode, cursor);

    const container = document.getElementById('messages');
    container.scrollTop = container.scrollHeight;
  }
}

function finalizeStream(convId, tokensIn, tokensOut) {
  // Remove cursor
  if (state.streamingBubble) {
    const cursor = state.streamingBubble.querySelector('.cursor');
    if (cursor) cursor.remove();
    state.streamingBubble = null;
  }
  state.streamingId = null;

  // Update live dot
  const entry = state.conversations[convId];
  if (entry && entry.liveDot) {
    entry.liveDot.classList.remove('visible');
  }

  // Add token info
  if (tokensIn != null || tokensOut != null) {
    const container = document.getElementById('messages');
    const meta = document.createElement('div');
    meta.className = 'msg-meta';
    meta.textContent = `Tokens: ${tokensIn ?? '?'} in / ${tokensOut ?? '?'} out`;
    container.appendChild(meta);
  }
}

// ─── Helpers ─────────────────────────────────────────────────────────────────

function updateCounter() {
  const total = Object.keys(state.conversations).length;
  document.getElementById('counter').textContent = `${total} conversation${total !== 1 ? 's' : ''}`;
}

function formatTime(iso) {
  if (!iso) return '';
  try {
    const d = new Date(iso);
    return d.toLocaleString();
  } catch {
    return iso;
  }
}

function escHtml(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// ─── Start ───────────────────────────────────────────────────────────────────
document.addEventListener('DOMContentLoaded', init);
