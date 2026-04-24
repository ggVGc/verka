(() => {
  const typeShapes = {
    description:    { shape: 'box',      color: '#6366f1' },
    task:           { shape: 'ellipse',  color: '#8b5cf6' },
    implementation: { shape: 'database', color: '#0ea5e9' },
    verification:   { shape: 'diamond',  color: '#14b8a6' },
    build:          { shape: 'hexagon',  color: '#f97316' },
    artifact:       { shape: 'dot',      color: '#64748b' },
  };

  const statusBorder = {
    draft:   { color: '#9ca3af', width: 1 },
    ready:   { color: '#3b82f6', width: 2 },
    running: { color: '#eab308', width: 3 },
    passed:  { color: '#10b981', width: 2 },
    failed:  { color: '#ef4444', width: 3 },
  };

  const edgeStyles = {
    child:             { color: '#4b5563', dashes: false, arrows: 'to' },
    depends_on:        { color: '#2563eb', dashes: false, arrows: 'to' },
    verifies:          { color: '#14b8a6', dashes: [4, 4], arrows: 'to' },
    builds:            { color: '#f97316', dashes: [4, 4], arrows: 'to' },
    consumes_artifact: { color: '#64748b', dashes: [2, 3], arrows: 'to' },
    supersedes:        { color: '#ef4444', dashes: [6, 4], arrows: 'to' },
  };

  let rawNodes = [];
  let rawEdges = [];
  let network = null;
  let nodesDS = null;
  let edgesDS = null;

  const container = document.getElementById('graph');
  const detailsEl = document.getElementById('details');
  const searchEl = document.getElementById('search');

  function shorten(id) { return id ? id.slice(0, 8) : ''; }

  function nodeLabel(n) {
    let title = '';
    try {
      const c = n.content ? JSON.parse(n.content) : null;
      if (c && typeof c === 'object') {
        title = c.title || c.name || c.summary || c.goal || '';
      }
    } catch (_) {}
    const head = title ? title.slice(0, 40) : n.type;
    return `${head}\n${shorten(n.id)}`;
  }

  function toVisNode(n) {
    const t = typeShapes[n.type] || { shape: 'box', color: '#999' };
    const border = statusBorder[n.status] || { color: '#888', width: 1 };
    return {
      id: n.id,
      label: nodeLabel(n),
      shape: t.shape,
      color: {
        background: t.color,
        border: n.stale ? '#b45309' : border.color,
        highlight: { background: t.color, border: '#000' },
      },
      borderWidth: n.stale ? 3 : border.width,
      shapeProperties: { borderDashes: n.stale ? [4, 3] : false },
      font: { color: '#fff', size: 12, multi: false, face: 'monospace' },
      margin: 8,
      _type: n.type,
      _status: n.status,
    };
  }

  function toVisEdge(e, idx) {
    const st = edgeStyles[e.kind] || { color: '#999', arrows: 'to' };
    return {
      id: `e${idx}`,
      from: e.src,
      to: e.dst,
      arrows: st.arrows,
      dashes: st.dashes || false,
      color: { color: st.color, highlight: st.color },
      label: e.kind,
      font: { size: 10, color: '#555', background: 'rgba(255,255,255,0.8)', align: 'middle' },
      smooth: { type: 'cubicBezier', forceDirection: 'vertical', roundness: 0.4 },
      _kind: e.kind,
    };
  }

  function activeTypes() {
    return new Set(Array.from(document.querySelectorAll('.typefilter:checked')).map(x => x.value));
  }
  function activeEdges() {
    return new Set(Array.from(document.querySelectorAll('.edgefilter:checked')).map(x => x.value));
  }

  function applyFilters() {
    if (!nodesDS) return;
    const ts = activeTypes();
    const es = activeEdges();
    const needle = searchEl.value.trim().toLowerCase();

    const visibleNodeIds = new Set();
    const updates = rawNodes.map(n => {
      let hide = !ts.has(n.type);
      if (!hide && needle) {
        const hay = (n.id + ' ' + (n.content || '')).toLowerCase();
        if (!hay.includes(needle)) hide = true;
      }
      if (!hide) visibleNodeIds.add(n.id);
      return { id: n.id, hidden: hide };
    });
    nodesDS.update(updates);

    edgesDS.update(rawEdges.map((e, i) => ({
      id: `e${i}`,
      hidden: !es.has(e.kind) || !visibleNodeIds.has(e.src) || !visibleNodeIds.has(e.dst),
    })));
  }

  async function loadGraph() {
    const res = await fetch('/api/graph');
    if (!res.ok) { alert('failed to load graph: ' + res.status); return; }
    const data = await res.json();
    rawNodes = data.nodes || [];
    rawEdges = data.edges || [];

    nodesDS = new vis.DataSet(rawNodes.map(toVisNode));
    edgesDS = new vis.DataSet(rawEdges.map(toVisEdge));

    if (network) network.destroy();
    network = new vis.Network(container, { nodes: nodesDS, edges: edgesDS }, {
      layout: {
        hierarchical: {
          enabled: true,
          direction: 'UD',
          sortMethod: 'directed',
          nodeSpacing: 160,
          levelSeparation: 140,
        },
      },
      physics: { enabled: false },
      interaction: { hover: true, navigationButtons: true, keyboard: true, tooltipDelay: 150 },
      edges: { smooth: { type: 'cubicBezier' } },
    });

    network.on('click', params => {
      if (params.nodes && params.nodes[0]) showNode(params.nodes[0]);
      else if (!params.edges.length) detailsEl.innerHTML = '<p class="hint">click a node to inspect</p>';
    });

    applyFilters();
  }

  async function showNode(id) {
    detailsEl.innerHTML = '<p class="hint">loading…</p>';
    try {
      const res = await fetch('/api/node/' + encodeURIComponent(id));
      if (!res.ok) { detailsEl.innerHTML = `<p class="hint">error: ${res.status}</p>`; return; }
      const n = await res.json();
      renderDetails(n);
    } catch (e) {
      detailsEl.innerHTML = `<p class="hint">error: ${e}</p>`;
    }
  }

  function renderDetails(n) {
    const parts = [];
    parts.push(`<div class="node-header">
      <h2>${escapeHTML(n.id)}</h2>
      <button class="close" title="Close" data-close>&times;</button>
    </div>`);

    parts.push(`<div class="badges">
      <strong>${escapeHTML(n.type)}</strong>
      <span class="chip s-${n.status}">${n.status}</span>
      ${n.stale ? '<span class="chip stale">stale</span>' : ''}
    </div>`);

    parts.push('<ul class="kv-list">');
    parts.push(`<li><span class="k">hash</span><span class="v">${escapeHTML(n.content_hash)}</span></li>`);
    parts.push(`<li><span class="k">created</span><span class="v">${new Date(n.created_at).toLocaleString()}</span></li>`);
    parts.push(`<li><span class="k">updated</span><span class="v">${new Date(n.updated_at).toLocaleString()}</span></li>`);
    parts.push('</ul>');

    if (n.content !== null && n.content !== undefined) {
      let parsed = n.content;
      if (typeof n.content === 'string') {
        try { parsed = JSON.parse(n.content); } catch (_) {}
      }
      parts.push('<div class="section-title">content</div>');
      parts.push(renderContent(parsed));
    }

    if (n.files && n.files.length) {
      parts.push('<div class="section-title">files</div><ul class="items">');
      for (const f of n.files) {
        parts.push(`<li><code>${escapeHTML(f.rel_path)}</code><span class="kind">${f.size} B</span></li>`);
      }
      parts.push('</ul>');
    }

    if (n.edges && n.edges.length) {
      parts.push('<div class="section-title">edges</div><ul class="items">');
      for (const e of n.edges) {
        const other = e.src === n.id ? e.dst : e.src;
        const dir = e.src === n.id ? '→' : '←';
        parts.push(`<li><span class="dir">${dir}</span><a class="ref" data-jump="${escapeHTML(other)}">${shorten(other)}</a><span class="kind">${escapeHTML(e.kind)}</span></li>`);
      }
      parts.push('</ul>');
    }

    if (n.latest_run) {
      const r = n.latest_run;
      parts.push('<div class="section-title">latest run</div>');
      parts.push('<ul class="kv-list">');
      parts.push(`<li><span class="k">kind</span><span class="v">${escapeHTML(r.kind)}</span></li>`);
      parts.push(`<li><span class="k">exit</span><span class="v">${r.exit_code ?? '—'}</span></li>`);
      parts.push(`<li><span class="k">started</span><span class="v">${new Date(r.started_at).toLocaleString()}</span></li>`);
      if (r.finished_at) parts.push(`<li><span class="k">finished</span><span class="v">${new Date(r.finished_at).toLocaleString()}</span></li>`);
      if (r.artifact_rel) parts.push(`<li><span class="k">artifact</span><span class="v">${escapeHTML(r.artifact_rel)}</span></li>`);
      parts.push('</ul>');
    }

    detailsEl.innerHTML = parts.join('');
    detailsEl.querySelectorAll('a[data-jump]').forEach(a => {
      a.addEventListener('click', () => {
        const id = a.getAttribute('data-jump');
        network.selectNodes([id]);
        network.focus(id, { scale: 1.2, animation: true });
        showNode(id);
      });
    });
    const closeBtn = detailsEl.querySelector('[data-close]');
    if (closeBtn) closeBtn.addEventListener('click', () => {
      detailsEl.innerHTML = '<p class="hint">click a node to inspect</p>';
    });
  }

  function renderContent(c) {
    if (c === null || c === undefined) return '';
    return `<div class="json-root">${renderJSON(c, 0)}</div>`;
  }

  function humanKey(k) {
    return String(k)
      .replace(/[_-]+/g, ' ')
      .replace(/\b\w/g, ch => ch.toUpperCase());
  }

  function renderJSON(v, depth) {
    if (v === null) return '<span class="j-null">null</span>';
    if (v === undefined) return '';
    const t = typeof v;
    if (t === 'string') {
      if (v.includes('\n') || v.length > 60) {
        return `<div class="j-string-long">${escapeHTML(v)}</div>`;
      }
      return `<span class="j-string">${escapeHTML(v)}</span>`;
    }
    if (t === 'number') return `<span class="j-num">${v}</span>`;
    if (t === 'boolean') return `<span class="j-bool">${v}</span>`;
    if (Array.isArray(v)) return renderArray(v, depth);
    if (t === 'object') return renderObject(v, depth);
    return escapeHTML(String(v));
  }

  function isPrimitive(v) {
    return v === null || v === undefined || ['string', 'number', 'boolean'].includes(typeof v);
  }

  function renderArray(arr, depth) {
    if (arr.length === 0) return '<span class="j-empty">(empty)</span>';
    if (arr.every(isPrimitive)) {
      const items = arr.map(v => `<li>${renderJSON(v, depth + 1)}</li>`).join('');
      return `<ul class="j-array-flat">${items}</ul>`;
    }
    const items = arr.map((v, i) => `
      <li class="j-array-item">
        <div class="j-array-index">${i}</div>
        <div class="j-array-body">${renderJSON(v, depth + 1)}</div>
      </li>`).join('');
    return `<ol class="j-array">${items}</ol>`;
  }

  function renderObject(obj, depth) {
    const keys = Object.keys(obj);
    if (keys.length === 0) return '<span class="j-empty">{}</span>';

    // For nested objects where all values are primitive, use compact key-value table.
    if (depth > 0 && keys.every(k => isPrimitive(obj[k]))) {
      const rows = keys.map(k => `
        <li>
          <span class="k">${escapeHTML(humanKey(k))}</span>
          <span class="v">${renderJSON(obj[k], depth + 1)}</span>
        </li>`).join('');
      return `<ul class="kv-list compact">${rows}</ul>`;
    }

    // Otherwise: each key gets a header, value rendered recursively.
    const Hdepth = Math.min(3 + depth, 6);
    const sections = keys.map(k => {
      const v = obj[k];
      const header = `<h${Hdepth} class="j-key">${escapeHTML(humanKey(k))}</h${Hdepth}>`;
      return `<section class="j-field">${header}<div class="j-value">${renderJSON(v, depth + 1)}</div></section>`;
    }).join('');
    return sections;
  }

  function escapeHTML(s) {
    return String(s).replace(/[&<>"']/g, c => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;'
    })[c]);
  }

  document.getElementById('refresh').addEventListener('click', loadGraph);
  document.querySelectorAll('.typefilter,.edgefilter').forEach(el => el.addEventListener('change', applyFilters));
  searchEl.addEventListener('input', applyFilters);

  loadGraph();
})();
