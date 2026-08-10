(function() {
  'use strict';

  var state = {
    runs: [],
    selectedRun: null,
    selectedRunData: null,
    events: [],
    activeFilter: 'all',
    activeTab: 'events',
    searchTerm: '',
    autoRefresh: true,
    refreshTimer: null
  };

  var REFRESH_INTERVAL = 3000;

  function api(path) {
    return fetch('/api/' + path).then(function(r) {
      if (!r.ok) throw new Error('HTTP ' + r.status);
      return r.json();
    });
  }

  function formatTokens(n) {
    if (n >= 1000000) return (n / 1000000).toFixed(1) + 'M';
    if (n >= 1000) return (n / 1000).toFixed(1) + 'k';
    return String(n);
  }

  function formatTime(iso) {
    if (!iso) return '--';
    var d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleString(undefined, {
      month: 'short', day: 'numeric',
      hour: '2-digit', minute: '2-digit', second: '2-digit'
    });
  }

  function shortId(id) {
    if (!id) return '--';
    if (id.length <= 12) return id;
    return id.substring(0, 8) + '...';
  }

  function statusClass(status) {
    var s = (status || '').toLowerCase();
    if (s.indexOf('run') === 0) return 'running';
    if (s.indexOf('complete') === 0) return 'completed';
    if (s.indexOf('fail') >= 0) return 'failed';
    if (s.indexOf('cancel') >= 0) return 'cancelled';
    return 'pending';
  }

  function statusLabel(status) {
    var s = (status || '').toLowerCase();
    if (s.indexOf('run') === 0) return 'Running';
    if (s.indexOf('complete') === 0) return 'Completed';
    if (s.indexOf('fail') >= 0) return 'Failed';
    if (s.indexOf('cancel') >= 0) return 'Cancelled';
    return status || 'Unknown';
  }

  // ── Run List Rendering ──

  function renderRunList() {
    var container = document.getElementById('run-list');
    var filtered = state.searchTerm
      ? state.runs.filter(function(r) {
          var q = state.searchTerm.toLowerCase();
          return (r.task || '').toLowerCase().indexOf(q) >= 0 ||
                 (r.run_dir || '').toLowerCase().indexOf(q) >= 0;
        })
      : state.runs;

    if (filtered.length === 0) {
      container.innerHTML = state.searchTerm
        ? '<div class="empty-state"><p>No matches</p><span>Try a different search</span></div>'
        : '<div class="empty-state"><p>No runs yet</p><span>Runs will appear here when workflows execute</span></div>';
      return;
    }

    var html = filtered.map(function(run) {
      var cls = statusClass(run.status);
      var active = state.selectedRun === run.run_dir ? ' active' : '';
      return '<div class="run-item' + active + '" data-run-dir="' + escAttr(run.run_dir) + '">' +
        '<div class="run-item-top">' +
          '<span class="run-status-dot ' + cls + '"></span>' +
          '<span class="run-task">' + escHtml(run.task || run.run_dir) + '</span>' +
        '</div>' +
        '<div class="run-meta">' +
          '<span>' + shortId(run.run_dir) + '</span>' +
          '<span>P:' + run.current_phase + '</span>' +
          '<span>A:' + run.completed_agents + '/' + run.total_started + '</span>' +
          '<span>' + formatTokens(run.total_tokens || 0) + ' tok</span>' +
        '</div>' +
      '</div>';
    }).join('');

    container.innerHTML = html;

    var items = container.querySelectorAll('.run-item');
    for (var i = 0; i < items.length; i++) {
      items[i].addEventListener('click', function() {
        selectRun(this.getAttribute('data-run-dir'));
      });
    }
  }

  // ── Run Detail Rendering ──

  function renderDetail(run) {
    document.getElementById('empty-detail').style.display = 'none';
    document.getElementById('run-detail').style.display = '';

    var badge = document.getElementById('detail-status-badge');
    var cls = statusClass(run.status);
    badge.className = 'status-badge ' + cls;
    badge.textContent = statusLabel(run.status);

    document.getElementById('detail-task').textContent = run.task || run.run_dir || '--';
    document.getElementById('detail-run-id').textContent = shortId(run.run_id);
    document.getElementById('detail-run-dir').textContent = run.run_dir || '--';
    document.getElementById('detail-created').textContent = formatTime(run.created_at);
    document.getElementById('detail-updated').textContent = formatTime(run.updated_at);

    document.getElementById('stat-phase').textContent = run.completed_phases + ' / ' + run.current_phase;
    document.getElementById('stat-agents').textContent = run.completed_agents + ' / ' + run.total_started;
    document.getElementById('stat-tokens').textContent = formatTokens(run.total_tokens || 0);
    document.getElementById('stat-running').textContent = run.running_agents || 0;
  }

  // ── Event Log Rendering ──

  function getEventType(evt) {
    if (evt.type) return evt.type;
    var keys = Object.keys(evt);
    for (var i = 0; i < keys.length; i++) {
      var k = keys[i];
      if (k === 'ts' || k === 'run_id') continue;
      var v = evt[k];
      if (v && typeof v === 'object') {
        if (v.status) return k;
      }
    }
    return keys[0] || 'Unknown';
  }

  function getEventMessage(evt, type) {
    var data = evt[type];
    if (!data) return JSON.stringify(evt, null, 2);

    var parts = [];
    if (data.agent_id) parts.push('agent: ' + shortId(data.agent_id));
    if (data.name) parts.push('name: ' + data.name);
    if (data.phase) parts.push('phase: ' + data.phase);
    if (data.message) parts.push(data.message);
    if (data.status) parts.push('status: ' + data.status);
    if (data.output) {
      var out = typeof data.output === 'string' ? data.output : JSON.stringify(data.output);
      if (out.length > 200) out = out.substring(0, 200) + '...';
      parts.push('output: ' + out);
    }
    if (data.reasoning) parts.push('reasoning: ' + data.reasoning);
    if (data.report !== undefined) {
      var rep = typeof data.report === 'string' ? data.report : JSON.stringify(data.report);
      parts.push('report: ' + rep);
    }
    if (data.level) parts.push('[' + data.level + ']');

    if (parts.length === 0) return JSON.stringify(data, null, 2);
    return parts.join(' | ');
  }

  function shouldShowEvent(type) {
    if (state.activeFilter === 'all') return true;
    if (state.activeFilter === 'error') {
      return type.indexOf('Fail') >= 0 || type.indexOf('Error') >= 0 || type.indexOf('Cancel') >= 0;
    }
    return type === state.activeFilter;
  }

  function renderEvents() {
    var container = document.getElementById('event-log');

    var filtered = state.events.filter(function(evt) {
      return shouldShowEvent(getEventType(evt));
    });

    if (filtered.length === 0) {
      container.innerHTML = '<div class="event-empty">No events</div>';
      return;
    }

    var html = filtered.slice().reverse().map(function(evt) {
      var type = getEventType(evt);
      var ts = evt.ts || '';
      var msg = getEventMessage(evt, type);
      return '<div class="event-row">' +
        '<span class="event-type ' + escAttr(type) + '">' + escHtml(type) + '</span>' +
        '<span class="event-ts">' + escHtml(formatTime(ts)) + '</span>' +
        '<span class="event-msg">' + escHtml(msg) + '</span>' +
      '</div>';
    }).join('');

    container.innerHTML = html;
  }

  // ── Actions ──

  function selectRun(runDir) {
    state.selectedRun = runDir;
    renderRunList();

    Promise.all([
      api('runs/' + encodeURIComponent(runDir)),
      api('runs/' + encodeURIComponent(runDir) + '/events'),
      api('runs/' + encodeURIComponent(runDir) + '/report').catch(function() { return null; })
    ]).then(function(results) {
      if (results[0]) {
        state.selectedRunData = results[0];
        renderDetail(results[0]);
      }
      state.events = results[1] || [];
      renderEvents();

      if (results[2]) {
        var reportEl = document.getElementById('report-output');
        if (results[2].found) {
          reportEl.textContent = JSON.stringify(results[2].value, null, 2);
        } else if (results[2].run_finished) {
          reportEl.textContent = 'Run finished (no report value)';
        } else {
          reportEl.textContent = 'No report available';
        }
      }
    }).catch(function(err) {
      console.error('Failed to load run details:', err);
    });
  }

  function refreshRuns() {
    api('runs').then(function(runs) {
      state.runs = runs || [];
      renderRunList();

      if (state.selectedRun && state.autoRefresh) {
        var found = state.runs.find(function(r) { return r.run_dir === state.selectedRun; });
        if (found) {
          renderDetail(found);
          if (found.status === 'running') {
            loadEvents(state.selectedRun);
          }
        }
      }
    }).catch(function(err) {
      console.error('Failed to refresh runs:', err);
    });
  }

  function loadEvents(runDir) {
    api('runs/' + encodeURIComponent(runDir) + '/events').then(function(events) {
      state.events = events || [];
      renderEvents();
    }).catch(function() {});
  }

  // ── Utilities ──

  function escHtml(s) {
    if (s == null) return '';
    return String(s)
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;');
  }

  function escAttr(s) {
    return escHtml(s);
  }

  function setupTabs() {
    var tabs = document.querySelectorAll('.tab');
    for (var i = 0; i < tabs.length; i++) {
      tabs[i].addEventListener('click', function() {
        var tab = this.getAttribute('data-tab');
        state.activeTab = tab;
        document.querySelectorAll('.tab').forEach(function(t) { t.classList.remove('active'); });
        this.classList.add('active');
        document.querySelectorAll('.tab-content').forEach(function(c) { c.style.display = 'none'; });
        document.getElementById('tab-' + tab).style.display = '';
      });
    }
  }

  function setupFilters() {
    var chips = document.querySelectorAll('.filter-chip');
    for (var i = 0; i < chips.length; i++) {
      chips[i].addEventListener('click', function() {
        document.querySelectorAll('.filter-chip').forEach(function(c) { c.classList.remove('active'); });
        this.classList.add('active');
        state.activeFilter = this.getAttribute('data-filter');
        renderEvents();
      });
    }
  }

  function setupAutoRefresh() {
    var checkbox = document.getElementById('auto-refresh');
    checkbox.addEventListener('change', function() {
      state.autoRefresh = checkbox.checked;
      if (state.autoRefresh) {
        startAutoRefresh();
      } else {
        stopAutoRefresh();
      }
    });
  }

  function startAutoRefresh() {
    stopAutoRefresh();
    state.refreshTimer = setInterval(refreshRuns, REFRESH_INTERVAL);
  }

  function stopAutoRefresh() {
    if (state.refreshTimer) {
      clearInterval(state.refreshTimer);
      state.refreshTimer = null;
    }
  }

  function init() {
    document.getElementById('search').addEventListener('input', function(e) {
      state.searchTerm = e.target.value;
      renderRunList();
    });

    document.getElementById('refresh-btn').addEventListener('click', refreshRuns);

    setupTabs();
    setupFilters();
    setupAutoRefresh();

    refreshRuns();
    startAutoRefresh();
  }

  if (document.readyState === 'loading') {
    document.addEventListener('DOMContentLoaded', init);
  } else {
    init();
  }
})();
