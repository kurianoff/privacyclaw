'use strict';

const state = {
  conversations: {},   // id → { conv, messages, element, liveDot }
  activeId: null,
  totalRequests: 0,
  ws: null,
  streamingId: null,
  streamingBubble: null,
  piiByConv: {},       // id → [ { type, original, original_masked, synthetic, tier, confidence } ]
  piiPanelOpen: true,
  compareMode: false,
};

// ─── Bootstrap ───────────────────────────────────────────────────────────────

async function init() {
  await loadConversations();
  connectWebSocket();
  updateProxyStatusOnLoad();

  document.getElementById('search').addEventListener('input', (e) => {
    filterConversations(e.target.value.toLowerCase());
  });

  document.getElementById('pii-panel-toggle').addEventListener('click', () => {
    state.piiPanelOpen = !state.piiPanelOpen;
    const body = document.getElementById('pii-panel-body');
    const chevron = document.getElementById('pii-chevron');
    body.classList.toggle('hidden', !state.piiPanelOpen);
    chevron.innerHTML = state.piiPanelOpen ? '&#9660;' : '&#9654;';
  });

  document.getElementById('compare-btn').addEventListener('click', () => {
    state.compareMode = !state.compareMode;
    document.getElementById('compare-btn').classList.toggle('active', state.compareMode);
    if (state.activeId) {
      const msgs = state.conversations[state.activeId]?.messages || [];
      renderMessages(state.activeId, msgs);
    }
  });

  // Settings panel open/close
  document.getElementById('settings-btn').addEventListener('click', () => {
    const panel = document.getElementById('settings-panel');
    panel.classList.toggle('open');
    if (panel.classList.contains('open')) {
      loadSettingsValues();
    }
  });

  document.getElementById('settings-close').addEventListener('click', () => {
    document.getElementById('settings-panel').classList.remove('open');
  });

  // PII mode change
  document.getElementById('pii-mode-select').addEventListener('change', async (e) => {
    const mode = e.target.value;
    const tier1 = mode !== 'off';
    const payload = { pii: { mode, tiers: { regex: tier1 } } };
    const result = await patchConfig(payload);
    if (result.restart_required) showRestartBanner();
    updateTierDependencies();
  });

  // Tier 1 toggle
  document.getElementById('tier1-toggle').addEventListener('change', async (e) => {
    const enable = e.target.checked;
    const payload = enable
      ? { pii: { tiers: { regex: true } } }
      : { pii: { tiers: { regex: false, ner: false, slm: false } } };
    await patchConfig(payload);
    updateTierDependencies();
  });

  // Tier 2 toggle
  document.getElementById('tier2-toggle').addEventListener('change', async (e) => {
    const enable = e.target.checked;
    const payload = enable
      ? { pii: { tiers: { regex: true, ner: true } } }
      : { pii: { tiers: { ner: false, slm: false } } };
    await patchConfig(payload);
    updateTierDependencies();
  });

  // Tier 3 toggle
  document.getElementById('tier3-toggle').addEventListener('change', async (e) => {
    const enable = e.target.checked;
    const payload = enable
      ? { pii: { tiers: { regex: true, ner: true, slm: true } } }
      : { pii: { tiers: { slm: false } } };
    await patchConfig(payload);
    updateTierDependencies();
  });

  // Proxy start/stop
  document.getElementById('proxy-start-stop').addEventListener('click', async () => {
    const isRunning = document.getElementById('proxy-start-stop').dataset.running === 'true';
    const endpoint = isRunning ? '/api/proxy/stop' : '/api/proxy/start';
    await fetch(endpoint, { method: 'POST' });
    // UI updates via WS proxy_status event
  });

  // Restart banner dismiss
  document.getElementById('restart-banner-dismiss').addEventListener('click', () => {
    document.getElementById('restart-banner').classList.add('hidden');
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
    const msgs = data.messages || [];
    if (state.conversations[convId]) {
      state.conversations[convId].messages = msgs;
    }
    renderMessages(convId, msgs);
  } catch (e) {
    console.error('Failed to load messages:', e);
  }
}

async function loadVault(convId) {
  try {
    const res = await fetch(`/api/conversations/${convId}/vault`);
    if (!res.ok) return;
    const entries = await res.json();
    if (!Array.isArray(entries) || entries.length === 0) return;
    for (const entry of entries) {
      // Avoid duplicates from vault vs live WS events
      addPiiEntry(convId, entry);
    }
    markConvHasPii(convId);
    if (convId === state.activeId) {
      renderPiiPanel(convId);
      // Show compare button when vault data is available.
      document.getElementById('compare-btn').classList.remove('hidden');
      // Refresh compare view if currently open.
      if (state.compareMode) {
        const msgs = state.conversations[convId]?.messages || [];
        renderMessages(convId, msgs);
      }
    }
  } catch (e) {
    console.error('Failed to load vault:', e);
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
        content_masked: msg.content_masked || null,
        pii_processed: msg.pii_processed != null ? msg.pii_processed : null,
      });
      break;

    case 'text_delta':
      appendDelta(msg.conversation_id, msg.text);
      break;

    case 'response_complete':
      finalizeStream(msg.conversation_id, msg.tokens_in, msg.tokens_out);
      break;

    case 'pii_detected':
      handlePiiDetected(msg);
      break;

    case 'proxy_status':
      updateProxyStatusUI(msg.running);
      break;

    case 'config_changed':
      if (document.getElementById('settings-panel').classList.contains('open')) {
        loadSettingsValues();
      }
      break;
  }
}

// ─── PII ─────────────────────────────────────────────────────────────────────

/// Build a stable deduplication key for a PII entry.
function piiEntryKey(e) {
  return `${e.type}|${e.original_masked}|${e.synthetic}`;
}

/// Add `entry` to the per-conversation PII list, ignoring duplicates.
function addPiiEntry(convId, entry) {
  if (!state.piiByConv[convId]) state.piiByConv[convId] = [];
  const existing = state.piiByConv[convId];
  const key = piiEntryKey(entry);
  if (!existing.some(e => piiEntryKey(e) === key)) {
    existing.push(entry);
  }
}

function handlePiiDetected(msg) {
  const convId = msg.conversation_id;
  if (!convId) return;

  const entry = {
    type: msg.entity_type,
    original: msg.original || '',
    original_masked: msg.original_masked,
    synthetic: msg.synthetic,
    tier: msg.tier,
    confidence: msg.confidence,
  };

  addPiiEntry(convId, entry);

  markConvHasPii(convId);

  if (convId === state.activeId) {
    renderPiiPanel(convId);
    document.getElementById('compare-btn').classList.remove('hidden');

    // Update summary bar live.
    const summaryEl = document.getElementById('conv-summary');
    if (summaryEl) {
      const vault = state.piiByConv[convId] || [];
      summaryEl.replaceWith(buildConvSummary(vault));
    }

    // Increment turn badge for the last turn chip.
    const turnNav = document.getElementById('turn-nav');
    if (turnNav) {
      const chips = turnNav.querySelectorAll('.turn-chip');
      if (chips.length > 0) {
        const last = chips[chips.length - 1];
        let badge = last.querySelector('.turn-badge');
        if (!badge) {
          badge = document.createElement('span');
          badge.className = 'turn-badge';
          badge.textContent = '0';
          last.appendChild(badge);
        }
        badge.textContent = String(Number(badge.textContent) + 1);
      }
    }
  }
}

function markConvHasPii(convId) {
  const entry = state.conversations[convId];
  if (!entry) return;
  if (!entry.element.querySelector('.pii-badge')) {
    const badge = document.createElement('span');
    badge.className = 'pii-badge';
    badge.title = 'PII detected';
    badge.textContent = 'PII';
    entry.element.querySelector('.conv-header').appendChild(badge);
  }
}

function renderPiiPanel(convId) {
  const panel = document.getElementById('pii-panel');
  const tbody = document.getElementById('pii-table-body');
  const countEl = document.getElementById('pii-count');
  const body = document.getElementById('pii-panel-body');
  const chevron = document.getElementById('pii-chevron');

  const entries = state.piiByConv[convId] || [];

  if (entries.length === 0) {
    panel.classList.add('hidden');
    return;
  }

  panel.classList.remove('hidden');
  countEl.textContent = entries.length;
  body.classList.toggle('hidden', !state.piiPanelOpen);
  chevron.innerHTML = state.piiPanelOpen ? '&#9660;' : '&#9654;';

  tbody.innerHTML = '';
  for (const e of entries) {
    const tr = document.createElement('tr');
    tr.innerHTML = `
      <td><span class="pii-type-badge">${escHtml(e.type || '')}</span></td>
      <td class="pii-original">${escHtml(e.original || e.original_masked || '')}</td>
      <td class="pii-synthetic">${escHtml(e.synthetic || '')}</td>
      <td>${escHtml(e.tier != null ? String(e.tier) : '')}</td>
      <td>${escHtml(e.confidence != null ? String(e.confidence) : '')}</td>
    `;
    tbody.appendChild(tr);
  }
}

// ─── Compare View ─────────────────────────────────────────────────────────────

/// Build the four labelled column elements for the compare grid.
/// Returns an array of the four body <div>s (the scrollable inner containers).
function buildCompareColumns() {
  const headers = [
    { label: 'Original Request', cls: 'col-req-orig'  },
    { label: 'Sent to LLM',      cls: 'col-req-masked' },
    { label: 'LLM Response',     cls: 'col-res-llm'    },
    { label: 'Delivered',        cls: 'col-res-final'  },
  ];
  return headers.map(({ label, cls }) => {
    const col = document.createElement('div');
    col.className = 'compare-col';
    const hdr = document.createElement('div');
    hdr.className = `compare-col-header ${cls}`;
    hdr.textContent = label;
    const body = document.createElement('div');
    body.className = 'compare-col-body';
    col.appendChild(hdr);
    col.appendChild(body);
    return body;
  });
}

/// Derive term lists and lookup maps from the vault for use in highlighting.
/// Returns `{ termsOrig, termsSynth, origToSynth, synthToMasked }`.
function buildVaultTermMaps(vault) {
  const termsOrig = vault
    .filter(v => v.original)
    .map(v => ({ text: v.original, tier: v.tier || null, synthetic: v.synthetic, masked: v.original_masked }));
  const termsSynth = vault
    .filter(v => v.synthetic)
    .map(v => ({ text: v.synthetic, tier: v.tier || null, masked: v.original_masked }));
  const origToSynth = {};
  const synthToMasked = {};
  for (const v of vault) {
    if (v.original && v.synthetic) origToSynth[v.original] = v.synthetic;
    if (v.synthetic && v.original_masked) synthToMasked[v.synthetic] = v.original_masked;
  }
  return { termsOrig, termsSynth, origToSynth, synthToMasked };
}

function renderCompareView(container, messages, vault, convId) {
  // Summary bar.
  container.appendChild(buildConvSummary(vault));

  // Turn navigator.
  const turns = buildTurns(messages);
  container.appendChild(buildTurnNav(turns, vault));

  const view = document.createElement('div');
  view.className = 'compare-view';

  const cols = buildCompareColumns();
  const { termsOrig, termsSynth, origToSynth, synthToMasked } = buildVaultTermMaps(vault);

  let turnIndex = 0;
  for (const msg of messages) {
    const isRequest = msg.direction === 'request' || msg.role === 'user' || msg.role === 'system';
    const isAssistant = msg.role === 'assistant';

    if (isRequest) {
      // Column 2: use content_masked when available, fallback to client-side approximation.
      let col2Text;
      let col2Approx = false;
      if (msg.content_masked != null) {
        col2Text = msg.content_masked;
      } else if (msg.pii_processed === false) {
        col2Text = msg.content;
      } else {
        col2Text = applyPiiMasking(msg.content, vault);
        col2Approx = true;
      }

      const b0 = buildCompareBubble(msg.role, msg.content, termsOrig, 'pii-orig',
        (term) => {
          const s = origToSynth[term] || '?';
          return `${term} \u2014 replaced with \u00a7${s}\u00a7`;
        });
      if (msg.role !== 'system') {
        b0.dataset.turn = turnIndex;
        turnIndex++;
      }

      const b1 = buildCompareBubble(msg.role, col2Text, termsSynth, 'pii-synth',
        (term) => {
          const m = synthToMasked[term] || term;
          return `synthetic for: ${m}`;
        });
      if (col2Approx) b1.appendChild(buildApproxBadge());

      // Click handler for detection sidebar.
      if (convId && msg.id) {
        const msgId = msg.id;
        b0.style.cursor = 'pointer';
        b0.addEventListener('click', () => renderDetectionSidebar(convId, msgId));
        b1.style.cursor = 'pointer';
        b1.addEventListener('click', () => renderDetectionSidebar(convId, msgId));
      }

      cols[0].appendChild(b0);
      cols[1].appendChild(b1);
      cols[2].appendChild(buildCompareEmpty());
      cols[3].appendChild(buildCompareEmpty());
    } else if (isAssistant) {
      const llmText = msg.content_masked || msg.content;
      cols[0].appendChild(buildCompareEmpty());
      cols[1].appendChild(buildCompareEmpty());
      cols[2].appendChild(buildCompareBubble('assistant', llmText, termsSynth, 'pii-synth',
        (term) => `synthetic for: ${synthToMasked[term] || term}`));
      cols[3].appendChild(buildCompareBubble('assistant', msg.content, termsOrig, 'pii-restored',
        (term) => {
          const s = origToSynth[term] || '?';
          return `restored: ${term} \u2190 \u00a7${s}\u00a7`;
        }));
    }
  }

  for (const body of cols) {
    view.appendChild(body.parentElement);
  }
  container.appendChild(view);

  setupScrollSync(cols);

  // Click outside sidebar closes it.
  view.addEventListener('click', (e) => {
    if (!e.target.closest('.compare-bubble')) {
      const sidebar = document.getElementById('detection-sidebar');
      if (sidebar) sidebar.classList.add('hidden');
    }
  });
}

function buildCompareBubble(role, text, highlightTerms, cssClass, tooltipFn) {
  const el = document.createElement('div');
  el.className = `compare-bubble ${role || 'user'}`;
  el.innerHTML = buildHighlightedHtml(text, highlightTerms, cssClass, tooltipFn);
  return el;
}

function buildApproxBadge() {
  const el = document.createElement('span');
  el.className = 'approx-badge';
  el.title = 'Client-side approximation — server masked content unavailable';
  el.textContent = 'approx';
  return el;
}

function buildCompareEmpty() {
  const el = document.createElement('div');
  el.className = 'compare-empty';
  return el;
}

function applyPiiMasking(text, vault) {
  let result = text;
  // Longer originals first to avoid partial replacements.
  const pairs = vault
    .filter(v => v.original && v.synthetic)
    .sort((a, b) => b.original.length - a.original.length);
  for (const { original, synthetic } of pairs) {
    result = result.split(original).join(synthetic);
  }
  return result;
}

function buildHighlightedHtml(text, terms, cssClass, tooltipFn) {
  if (!terms || terms.length === 0) return escHtml(text);
  // Find all non-overlapping occurrences.
  const ranges = [];
  for (const { text: term, tier } of terms) {
    if (!term) continue;
    let idx = 0;
    while ((idx = text.indexOf(term, idx)) !== -1) {
      ranges.push({ start: idx, end: idx + term.length, tier, term });
      idx += term.length;
    }
  }
  if (ranges.length === 0) return escHtml(text);
  ranges.sort((a, b) => a.start - b.start || b.end - a.end);
  // Eliminate overlaps.
  const merged = [];
  let lastEnd = 0;
  for (const r of ranges) {
    if (r.start < lastEnd) continue;
    merged.push(r);
    lastEnd = r.end;
  }
  let html = '';
  let pos = 0;
  for (const r of merged) {
    html += escHtml(text.slice(pos, r.start));
    const tc = r.tier ? `tier-${r.tier}` : '';
    const extra = cssClass ? ` ${cssClass}` : '';
    const badge = r.tier ? `<span class="tier-badge ${tc}">T${r.tier}</span>` : '';
    const title = tooltipFn ? ` title="${escHtml(tooltipFn(r.term))}"` : '';
    html += `<mark class="pii-hl ${tc}${extra}"${title}>${badge}${escHtml(text.slice(r.start, r.end))}</mark>`;
    pos = r.end;
  }
  html += escHtml(text.slice(pos));
  return html;
}

// ─── Turn Navigator ────────────────────────────────────────────────────────

function buildTurns(messages) {
  const turns = [];
  let current = null;
  for (const msg of messages) {
    const isSystem = msg.role === 'system';
    const isRequest = msg.direction === 'request' || msg.role === 'user';
    const isAssistant = msg.role === 'assistant';
    if (isSystem) continue; // skip system messages in turn numbering
    if (isRequest) {
      current = { index: turns.length, requests: [msg], responses: [] };
      turns.push(current);
    } else if (isAssistant && current) {
      current.responses.push(msg);
    }
  }
  return turns;
}

function buildTurnNav(turns, vault) {
  const nav = document.createElement('div');
  nav.className = 'turn-nav';
  nav.id = 'turn-nav';

  for (const turn of turns) {
    const chip = document.createElement('button');
    chip.className = 'turn-chip';
    chip.textContent = `T${turn.index + 1}`;
    chip.addEventListener('click', () => scrollToTurn(turn.index));

    // Count detections for this turn's requests.
    const requestContents = turn.requests.map(r => r.content);
    const detectionCount = vault.filter(v =>
      requestContents.some(c => c && c.includes(v.original || v.original_masked))
    ).length;
    if (detectionCount > 0) {
      const badge = document.createElement('span');
      badge.className = 'turn-badge';
      badge.textContent = String(detectionCount);
      chip.appendChild(badge);
    }

    nav.appendChild(chip);
  }

  return nav;
}

function scrollToTurn(index) {
  const el = document.querySelector(`[data-turn="${index}"]`);
  if (el) el.scrollIntoView({ behavior: 'smooth', block: 'start' });
}

// ─── Detection Sidebar ─────────────────────────────────────────────────────

/// Render an array of detection objects as an HTML table string.
function buildDetectionTableHtml(detections) {
  let html = '<table class="sidebar-table"><thead><tr><th>Type</th><th>Masked</th><th>Synthetic</th><th>Tier</th><th>Confidence</th></tr></thead><tbody>';
  for (const d of detections) {
    const confPct = Math.round((d.confidence || 0) * 100);
    html += `<tr>
      <td><span class="pii-type-badge">${escHtml(d.entity_type || '')}</span></td>
      <td class="pii-masked">${escHtml(d.original_masked || '')}</td>
      <td class="pii-synthetic">${escHtml(d.synthetic || '')}</td>
      <td>${escHtml(d.tier != null ? `T${d.tier}` : '')}</td>
      <td><progress max="100" value="${confPct}"></progress> ${confPct}%</td>
    </tr>`;
  }
  html += '</tbody></table>';
  return html;
}

async function renderDetectionSidebar(convId, messageId) {
  const sidebar = document.getElementById('detection-sidebar');
  if (!sidebar) return;

  sidebar.classList.remove('hidden');
  sidebar.innerHTML = '<div class="sidebar-loading">Loading…</div>';

  try {
    const url = messageId
      ? `/api/conversations/${convId}/detections?message_id=${encodeURIComponent(messageId)}`
      : `/api/conversations/${convId}/detections`;
    const res = await fetch(url);
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    const detections = await res.json();

    if (!Array.isArray(detections) || detections.length === 0) {
      sidebar.innerHTML = '<div class="sidebar-empty">No detections for this message</div>';
      return;
    }

    sidebar.innerHTML = buildDetectionTableHtml(detections);
  } catch (e) {
    sidebar.innerHTML = `<div class="sidebar-error">Failed to load: ${escHtml(e.message)}</div>`;
  }
}

// ─── Conversation Summary Bar ──────────────────────────────────────────────

function buildConvSummary(vault) {
  const el = document.createElement('div');
  el.className = 'conv-summary';
  el.id = 'conv-summary';

  if (vault.length === 0) {
    el.textContent = 'No PII detected';
    return el;
  }

  const byType = {};
  for (const v of vault) {
    byType[v.type] = (byType[v.type] || 0) + 1;
  }

  const parts = Object.entries(byType).map(([type, count]) =>
    `<span class="pii-type-badge">${escHtml(type)}</span> ×${count}`
  ).join(' ');

  el.innerHTML = `<strong>${vault.length}</strong> PII entities detected: ${parts}`;
  return el;
}

function setupScrollSync(scrollables) {
  let syncing = false;
  for (const el of scrollables) {
    el.addEventListener('scroll', () => {
      if (syncing) return;
      syncing = true;
      const maxSrc = el.scrollHeight - el.clientHeight;
      const pct = maxSrc > 0 ? el.scrollTop / maxSrc : 0;
      for (const other of scrollables) {
        if (other === el) continue;
        const maxDst = other.scrollHeight - other.clientHeight;
        other.scrollTop = pct * maxDst;
      }
      syncing = false;
    });
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

  // Hide compare button until vault confirms PII data
  document.getElementById('compare-btn').classList.add('hidden');
  document.getElementById('compare-btn').classList.remove('active');
  state.compareMode = false;

  // Reset PII panel then load vault
  document.getElementById('pii-panel').classList.add('hidden');
  document.getElementById('pii-table-body').innerHTML = '';
  document.getElementById('pii-count').textContent = '0';
  renderPiiPanel(id);
  loadVault(id);
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

  if (state.compareMode) {
    const vault = state.piiByConv[convId] || [];
    container.classList.add('compare-mode');
    renderCompareView(container, messages, vault, convId);
  } else {
    container.classList.remove('compare-mode');
    for (const msg of messages) {
      const block = buildMessageBlock(msg.role, msg.content, msg.timestamp);
      container.appendChild(block);
    }
    container.scrollTop = container.scrollHeight;
  }
}

function appendMessage(convId, msg) {
  // Store in conversation state regardless of active view.
  if (state.conversations[convId]) {
    state.conversations[convId].messages.push(msg);
  }

  if (convId !== state.activeId) return;
  if (state.compareMode) return; // live messages don't update compare view

  const container = document.getElementById('messages');
  container.classList.remove('compare-mode');
  const block = buildMessageBlock(msg.role, msg.content, msg.timestamp);
  container.appendChild(block);
  container.scrollTop = container.scrollHeight;

  // Update turn nav live for request messages.
  if (msg.direction === 'request') {
    const vault = state.piiByConv[convId] || [];
    const msgs = state.conversations[convId]?.messages || [];
    const turns = buildTurns(msgs);
    const navEl = document.getElementById('turn-nav');
    if (navEl) {
      const newNav = buildTurnNav(turns, vault);
      navEl.replaceWith(newNav);
    }
  }
}

function buildMessageBlock(role, content, timestamp) {
  const block = document.createElement('div');
  const roleClass = role === 'user' ? 'user' : role === 'system' ? 'system' : 'assistant';
  block.className = `msg-block ${roleClass}`;

  const header = document.createElement('div');
  header.className = 'msg-header';

  const roleLabel = document.createElement('span');
  roleLabel.className = 'msg-role';
  roleLabel.textContent = role;

  const timeLabel = document.createElement('span');
  timeLabel.className = 'msg-time';
  timeLabel.textContent = formatShortTime(timestamp || new Date().toISOString());

  header.appendChild(roleLabel);
  header.appendChild(timeLabel);

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

  block.appendChild(header);
  block.appendChild(bubble);
  return block;
}

function appendDelta(convId, text) {
  if (convId !== state.activeId) return;

  if (state.streamingId !== convId) {
    // Start new streaming block
    const container = document.getElementById('messages');
    const block = buildMessageBlock('assistant', '', new Date().toISOString());
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

// ─── Settings Panel ──────────────────────────────────────────────────────────

async function loadSettingsValues() {
  try {
    const res = await fetch('/api/config');
    const cfg = await res.json();
    document.getElementById('pii-mode-select').value = cfg.pii.mode;
    document.getElementById('tier1-toggle').checked = cfg.pii.tiers.regex;
    document.getElementById('tier2-toggle').checked = cfg.pii.tiers.ner;
    document.getElementById('tier3-toggle').checked = cfg.pii.tiers.slm;
    updateTierDependencies();
  } catch (e) {
    console.error('Failed to load config:', e);
  }

  try {
    const statusRes = await fetch('/api/proxy/status');
    const status = await statusRes.json();
    updateProxyStatusUI(status.running);
  } catch (e) {
    console.error('Failed to load proxy status:', e);
  }

  loadModels();
}

async function patchConfig(payload) {
  try {
    const res = await fetch('/api/config', {
      method: 'PATCH',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(payload),
    });
    const result = await res.json();
    if (result.restart_required) showRestartBanner();
    return result;
  } catch (e) {
    console.error('Failed to patch config:', e);
    return {};
  }
}

function updateTierDependencies() {
  const t1 = document.getElementById('tier1-toggle').checked;
  const t2 = document.getElementById('tier2-toggle').checked;
  document.getElementById('tier2-toggle').disabled = !t1;
  document.getElementById('tier3-toggle').disabled = !(t1 && t2);
}

function showRestartBanner() {
  document.getElementById('restart-banner').classList.remove('hidden');
}

function updateProxyStatusUI(running) {
  const btn = document.getElementById('proxy-start-stop');
  const dot = document.getElementById('proxy-status-dot');
  btn.textContent = running ? 'Stop Proxy' : 'Start Proxy';
  btn.dataset.running = running ? 'true' : 'false';
  dot.className = 'proxy-status-dot ' + (running ? 'running' : 'stopped');
}

async function updateProxyStatusOnLoad() {
  try {
    const statusRes = await fetch('/api/proxy/status');
    const status = await statusRes.json();
    updateProxyStatusUI(status.running);
  } catch (e) {
    console.error('Failed to load proxy status on init:', e);
  }
}

async function loadModels() {
  try {
    const res = await fetch('/api/models');
    const models = await res.json();
    const tbody = document.getElementById('model-table-body');
    tbody.innerHTML = '';
    for (const m of models) {
      const tr = document.createElement('tr');
      tr.innerHTML = `
        <td>${escHtml(m.name)}</td>
        <td>${m.size_mb} MB</td>
        <td>${m.downloaded ? (m.active ? '<span class="active-badge">Active</span>' : 'Downloaded') : 'Not installed'}</td>
        <td>
          ${!m.downloaded ? `<button class="btn-small" onclick="downloadModel('${m.id}')">Download</button>` : ''}
          ${m.downloaded && !m.active ? `<button class="btn-small" onclick="activateModel('${m.id}')">Activate</button>` : ''}
          ${m.downloaded ? `<button class="btn-small btn-danger" onclick="deleteModel('${m.id}')">Delete</button>` : ''}
        </td>
      `;
      tbody.appendChild(tr);
    }
  } catch (e) {
    console.error('Failed to load models:', e);
  }
}

async function downloadModel(id) {
  await fetch(`/api/models/${id}/download`, { method: 'POST' });
  loadModels();
}

async function activateModel(id) {
  await fetch(`/api/models/${id}/activate`, { method: 'POST' });
  loadModels();
}

async function deleteModel(id) {
  await fetch(`/api/models/${id}`, { method: 'DELETE' });
  loadModels();
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

function formatShortTime(iso) {
  if (!iso) return '';
  try {
    const d = new Date(iso);
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  } catch {
    return '';
  }
}

function escHtml(s) {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

// ─── Start ───────────────────────────────────────────────────────────────────
document.addEventListener('DOMContentLoaded', init);
