import { useCallback, useEffect, useRef, useState } from 'react';
import { Terminal as XTerminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { listen } from '@tauri-apps/api/event';
import '@xterm/xterm/css/xterm.css';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import './App.css';

type Section = 'Overview' | 'File access' | 'Terminal' | 'Processes' | 'MCP server' | 'Permissions' | 'Activity';
type Entry = { name: string; path: string; is_dir: boolean };
type McpInfo = { command: string; binary_exists: boolean; transport: string; tools: string[] };
type ProcessInfo = { pid: number; parent_pid: number; name: string; state: string; resident_memory_bytes: number | null; protected: boolean; protection_reason: string | null };
type ProcessSnapshot = { processes: ProcessInfo[]; truncated: boolean };
type AuditEvent = { timestamp_unix_seconds: number; source: string; action: string; target: string | null; outcome: string };

function formatMemory(bytes: number | null) {
  if (bytes === null) return '—';
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
const sections: { label: Section; icon: string }[] = [
  { label: 'Overview', icon: '◫' }, { label: 'File access', icon: '▤' },
  { label: 'Terminal', icon: '>_' }, { label: 'Processes', icon: '▦' }, { label: 'MCP server', icon: '⌘' },
  { label: 'Permissions', icon: '◇' }, { label: 'Activity', icon: '◷' },
];

function InteractiveTerminal({ cwd, reportError }: { cwd: string; reportError: (message: string) => void }) {
  const host = useRef<HTMLDivElement>(null);
  const [sessionId, setSessionId] = useState('');
  const [terminalState, setTerminalState] = useState('Starting terminal…');
  const terminalRef = useRef<XTerminal | null>(null);

  useEffect(() => {
    if (!host.current || !cwd) return;
    let alive = true;
    let id = '';
    let unlistenOutput: (() => void) | undefined;
    let unlistenExit: (() => void) | undefined;
    const terminal = new XTerminal({ cursorBlink: true, convertEol: true, scrollback: 1500, fontSize: 12, fontFamily: "'DM Mono', monospace", theme: { background: '#0d1016', foreground: '#c5cbd5', cursor: '#c7e97b' } });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host.current);
    terminalRef.current = terminal;
    const sendResize = () => { if (id) void invoke('terminal_resize', { sessionId: id, cols: terminal.cols, rows: terminal.rows }).catch((e) => reportError(String(e))); };
    const resizeObserver = new ResizeObserver(() => { fit.fit(); sendResize(); });
    resizeObserver.observe(host.current);
    terminal.onData((data) => { if (id) void invoke('terminal_write', { sessionId: id, data }).catch((e) => reportError(String(e))); });
    void (async () => {
      try {
        unlistenOutput = await listen<{session_id: string; data: string}>('terminal-output', (event) => {
          if (event.payload.session_id === id) terminal.write(event.payload.data);
        });
        unlistenExit = await listen<{session_id: string}>('terminal-exited', (event) => {
          if (event.payload.session_id === id && alive) { setTerminalState('Shell exited'); setSessionId(''); void invoke('terminal_close', { sessionId: id }).catch((e) => reportError(String(e))); }
        });
        fit.fit();
        const started = await invoke<{session_id: string}>('terminal_start', { cwd, cols: terminal.cols, rows: terminal.rows });
        id = started.session_id;
        if (!alive) { await invoke('terminal_close', { sessionId: id }); return; }
        setSessionId(id);
        setTerminalState('Connected');
        sendResize();
        terminal.focus();
      } catch (e) { if (alive) { setTerminalState('Could not start'); reportError(String(e)); } }
    })();
    return () => {
      alive = false;
      resizeObserver.disconnect();
      unlistenOutput?.(); unlistenExit?.();
      if (id) void invoke('terminal_close', { sessionId: id });
      terminal.dispose(); terminalRef.current = null;
    };
  }, [cwd, reportError]);

  return <div className="interactive-terminal-wrap"><div className="interactive-terminal-bar"><span><i className="status-dot" /> {terminalState}</span><button disabled={!sessionId} onClick={() => { if (sessionId) void invoke('terminal_close', { sessionId }).then(() => { setSessionId(''); setTerminalState('Closed'); }).catch((e) => reportError(String(e))); }}>Close session</button></div><div ref={host} className="interactive-terminal" /></div>;
}

function App() {
  const [active, setActive] = useState<Section>('Overview');
  const [folders, setFolders] = useState<string[]>([]);
  const [writableFolders, setWritableFolders] = useState<string[]>([]);
  const [currentPath, setCurrentPath] = useState('');
  const [entries, setEntries] = useState<Entry[]>([]);
  const [preview, setPreview] = useState<{ path: string; content: string } | null>(null);
  const [previewDirty, setPreviewDirty] = useState(false);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [terminalCwd, setTerminalCwd] = useState('');
  const [mcpInfo, setMcpInfo] = useState<McpInfo | null>(null);
  const [commandExecutionEnabled, setCommandExecutionEnabled] = useState(false);
  const [processSnapshot, setProcessSnapshot] = useState<ProcessSnapshot | null>(null);
  const [processBusy, setProcessBusy] = useState(false);
  const [activity, setActivity] = useState<AuditEvent[]>([]);
  const [activityBusy, setActivityBusy] = useState(false);

  useEffect(() => {
    let cancelled = false;
    void invoke<string[]>('approved_folders')
      .then((approved) => { if (!cancelled) setFolders(approved); })
      .catch((e) => { if (!cancelled) setError(String(e)); });
    void invoke<string[]>('writable_folders')
      .then((approved) => { if (!cancelled) setWritableFolders(approved); })
      .catch((e) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, []);
  const refreshProcesses = useCallback(async () => {
    setProcessBusy(true);
    setError('');
    try { setProcessSnapshot(await invoke<ProcessSnapshot>('list_processes')); }
    catch (e) { setError(String(e)); }
    finally { setProcessBusy(false); }
  }, []);
  const refreshActivity = useCallback(async () => {
    setActivityBusy(true);
    setError('');
    try { setActivity(await invoke<AuditEvent[]>('recent_activity', { limit: 100 })); }
    catch (e) { setError(String(e)); }
    finally { setActivityBusy(false); }
  }, []);
  const terminateProcess = async (pid: number, name: string) => {
    if (!window.confirm('Send SIGTERM to ' + name + ' (PID ' + pid + ')? Unsaved work in that process may be lost.')) return;
    setError('');
    try {
      await invoke('terminate_process', { pid, confirm: true });
      await new Promise((resolve) => setTimeout(resolve, 250));
      await refreshProcesses();
    } catch (e) { setError(String(e)); }
  };
  useEffect(() => {
    if (active !== 'MCP server') return;
    let cancelled = false;
    void invoke<McpInfo>('mcp_server_info')
      .then((info) => { if (!cancelled) setMcpInfo(info); })
      .catch((e) => { if (!cancelled) setError(String(e)); });
    void invoke<boolean>('command_execution_enabled')
      .then((enabled) => { if (!cancelled) setCommandExecutionEnabled(enabled); })
      .catch((e) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, [active]);
  const toggleCommandExecution = async () => {
    const enabled = !commandExecutionEnabled;
    if (enabled && !window.confirm('Allow connected MCP clients to start programs on this computer as your Linux user? Commands are not sandboxed and can access anything your account can access.')) return;
    setError('');
    try {
      setCommandExecutionEnabled(await invoke<boolean>('set_command_execution_enabled', { enabled }));
    } catch (e) { setError(String(e)); }
  };

  const effectiveTerminalCwd = folders.includes(terminalCwd) ? terminalCwd : (folders[0] ?? '');
  const reportTerminalError = useCallback((message: string) => setError(message), []);
  const activeApprovedRoot = folders.find((root) => currentPath === root || currentPath.startsWith(root.endsWith('/') ? root : `${root}/`));
  const parentPath = currentPath ? currentPath.slice(0, currentPath.lastIndexOf('/')) || '/' : '';
  const parentBoundary = activeApprovedRoot ? (activeApprovedRoot.endsWith('/') ? activeApprovedRoot : `${activeApprovedRoot}/`) : '';
  const canNavigateUp = Boolean(activeApprovedRoot && currentPath !== activeApprovedRoot && (parentPath === activeApprovedRoot || parentPath.startsWith(parentBoundary)));

  const addFolder = async () => {
    setError('');
    const chosen = await open({ directory: true, multiple: false, title: 'Approve a folder for access' });
    if (!chosen || Array.isArray(chosen)) return;
    setBusy(true);
    try {
      const approved = await invoke<string[]>('approve_folder', { path: chosen });
      setFolders(approved);
      setCurrentPath(chosen);
      setEntries(await invoke<Entry[]>('list_folder', { path: chosen }));
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };

  const toggleWriteAccess = async (path: string) => {
    const enabled = !writableFolders.includes(path);
    if (enabled && !window.confirm('Allow connected AI clients to create, edit, move, and delete files inside this folder?\n\n' + path)) return;
    setError('');
    try {
      setWritableFolders(await invoke<string[]>('set_folder_write_access', { path, enabled }));
    } catch (e) { setError(String(e)); }
  };

  const openFolder = async (path: string) => {
    setError(''); setPreview(null); setBusy(true);
    try { setEntries(await invoke<Entry[]>('list_folder', { path })); setCurrentPath(path); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  };

  const openFile = async (path: string) => {
    setError(''); setBusy(true);
    try {
      const content = await invoke<string>('read_text_file', { path });
      setPreview({ path, content });
      setPreviewDirty(false);
    } catch (e) { setError(String(e)); setPreview(null); }
    finally { setBusy(false); }
  };



  const approvedWriteAccess = Boolean(activeApprovedRoot && writableFolders.includes(activeApprovedRoot));
  const buildChildPath = (name: string) => {
    const clean = name.trim();
    if (!clean || clean === '.' || clean === '..' || clean.includes('/') || clean.includes('\\')) {
      throw new Error('Enter a simple file or folder name without path separators.');
    }
    return currentPath.replace(/\/$/, '') + '/' + clean;
  };
  const createTextFile = async () => {
    const name = window.prompt('New text file name');
    if (!name) return;
    setError('');
    try {
      await invoke('write_text_file', { path: buildChildPath(name), content: '', overwrite: false });
      setEntries(await invoke<Entry[]>('list_folder', { path: currentPath }));
    } catch (e) { setError(String(e)); }
  };
  const createSubfolder = async () => {
    const name = window.prompt('New folder name');
    if (!name) return;
    setError('');
    try {
      await invoke('create_directory', { path: buildChildPath(name) });
      setEntries(await invoke<Entry[]>('list_folder', { path: currentPath }));
    } catch (e) { setError(String(e)); }
  };
  const savePreview = async () => {
    if (!preview) return;
    setError('');
    try {
      await invoke('write_text_file', { path: preview.path, content: preview.content, overwrite: true });
      setPreviewDirty(false);
    } catch (e) { setError(String(e)); }
  };
  const deletePreview = async () => {
    if (!preview || !window.confirm('Permanently delete ' + preview.path + '?')) return;
    setError('');
    try {
      await invoke('delete_file', { path: preview.path, confirm: true });
      setPreview(null);
      setPreviewDirty(false);
      setEntries(await invoke<Entry[]>('list_folder', { path: currentPath }));
    } catch (e) { setError(String(e)); }
  };
  const copyPreview = async () => {
    if (!preview) return;
    const currentName = preview.path.split('/').pop() ?? '';
    const name = window.prompt('Copy file as', currentName + '.copy');
    if (!name) return;
    setError('');
    try {
      const destination = buildChildPath(name);
      await invoke('copy_file', { source: preview.path, destination, confirm: true });
      setEntries(await invoke<Entry[]>('list_folder', { path: currentPath }));
    } catch (e) { setError(String(e)); }
  };
  const renamePreview = async () => {
    if (!preview) return;
    const name = window.prompt('New file name', preview.path.split('/').pop() ?? '');
    if (!name || name === preview.path.split('/').pop()) return;
    setError('');
    try {
      const destination = buildChildPath(name);
      if (!window.confirm('Move this file to ' + destination + '?')) return;
      await invoke('move_file', { source: preview.path, destination, confirm: true });
      setPreview({ ...preview, path: destination });
      setEntries(await invoke<Entry[]>('list_folder', { path: currentPath }));
    } catch (e) { setError(String(e)); }
  };

  const revokeFolder = async (path: string) => {
    setError('');
    try {
      setFolders(await invoke<string[]>('revoke_folder', { path }));
      setWritableFolders((current) => current.filter((root) => root !== path));
      if (currentPath === path || currentPath.startsWith(`${path}/`)) { setCurrentPath(''); setEntries([]); setPreview(null); }
    } catch (e) { setError(String(e)); }
  };

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand"><div className="brand-mark">S</div><div><strong>Sam's Desktop</strong><span>COMMANDER</span></div></div>
        <div className="nav-label">WORKSPACE</div>
        <nav aria-label="Main navigation">{sections.map((section) => <button key={section.label} className={`nav-item ${active === section.label ? 'selected' : ''}`} onClick={() => { setActive(section.label); if (section.label === 'Processes') void refreshProcesses(); if (section.label === 'Activity') void refreshActivity(); }}><span className="nav-icon">{section.icon}</span>{section.label}</button>)}</nav>
        <div className="sidebar-bottom"><span className="status-dot" /> Local machine <span className="version">v0.1.0</span></div>
      </aside>
      <main className="main-area">
        <header className="topbar"><div className="breadcrumb">Workspace <span>/</span> {active}</div><div className="local-pill"><span className="status-dot" /> LOCAL ONLY</div></header>
        <section className="page-content">
          <div className="page-heading"><div><p className="eyebrow">PERSONAL WORKSPACE</p><h1>{active}</h1><p className="subheading">Your computer, connected on your terms.</p></div><div className="date-chip">LINUX MINT <span>•</span> DESKTOP</div></div>
          {active === 'Overview' ? <>
            <div className="welcome-card"><div><div className="welcome-kicker">YOUR LOCAL CONTROL CENTER</div><h2>Welcome to your workspace.</h2><p>One place to manage local tools, access, and AI connections. You are in control of what gets permission.</p><div className="welcome-actions"><button className="primary-button" onClick={() => setActive('File access')}>Manage folders <span>↗</span></button><button className="text-button" onClick={() => setActive('MCP server')}>Configure MCP server</button></div></div><div className="orbit-art"><div className="orbit orbit-one"/><div className="orbit orbit-two"/><div className="orbit-core">S</div><span className="orbit-node node-one"/><span className="orbit-node node-two"/><span className="orbit-node node-three"/></div></div>
            <div className="section-title"><h3>System overview</h3><span>LOCAL STATUS</span></div>
            <div className="stats-grid"><article className="stat-card"><div className="stat-top"><span>Application</span><span className="mini-icon">▣</span></div><div className="stat-value">Running <i className="status-dot"/></div><p>Desktop shell initialized</p></article><article className="stat-card"><div className="stat-top"><span>MCP server</span><span className="mini-icon">⌘</span></div><div className="stat-value">Client-launched</div><p>STDIO process starts when an AI client connects</p></article><article className="stat-card"><div className="stat-top"><span>Approved folders</span><span className="mini-icon">▤</span></div><div className="stat-value">{folders.length} <small>folders</small></div><p>Write access is separately controlled</p></article><article className="stat-card"><div className="stat-top"><span>Remote access</span><span className="mini-icon">◇</span></div><div className="stat-value">Disabled</div><p>This application stays local</p></article></div>
            <div className="bottom-grid"><article className="panel"><div className="panel-heading"><h3>Getting started</h3><span>01 / 04</span></div><div className="step-row"><span className="step-number">01</span><div><strong>Set your access boundaries</strong><p>Choose which folders the application can use.</p></div><button onClick={() => setActive('File access')}>→</button></div><div className="step-row"><span className="step-number">02</span><div><strong>Configure the MCP server</strong><p>Expose approved tools to a compatible AI client.</p></div><button onClick={() => setActive('MCP server')}>→</button></div><div className="step-row"><span className="step-number">03</span><div><strong>Review activity</strong><p>Inspect actions and permission decisions.</p></div><button onClick={() => setActive('Activity')}>→</button></div></article><article className="panel security-panel"><div className="panel-heading"><h3>Security posture</h3><span className="safe-label">DEFAULT SAFE</span></div><div className="security-symbol">✓</div><h4>Local-first by design</h4><p>No remote listener is enabled. Folder listing is limited to explicitly approved roots.</p><div className="security-rule"><span>Network exposure</span><strong>None</strong></div><div className="security-rule"><span>File changes</span><strong>Separate approval</strong></div></article></div>
          </> : active === 'File access' ? <div className="workspace-panel">
            <div className="workspace-toolbar"><div><h2>Approved folders</h2><p>Only approved folders are visible. File changes require a separate approval for each folder.</p></div><button className="primary-button" disabled={busy} onClick={() => void addFolder()}>+ Approve folder</button></div>
            {error && <p className="error-message" role="alert">{error}</p>}
            <div className="folder-layout"><div className="folder-list"><h3>Access roots <span>{folders.length}</span></h3>{folders.length === 0 ? <p className="muted-copy">No folders approved yet.</p> : folders.map((folder) => <div className="folder-row" key={folder}><button className="folder-select" onClick={() => void openFolder(folder)}>▰ <span>{folder}</span></button><button className="write-toggle" onClick={() => void toggleWriteAccess(folder)}>{writableFolders.includes(folder) ? 'Disable changes' : 'Enable changes'}</button><button className="revoke-button" onClick={() => void revokeFolder(folder)}>Revoke</button></div>)}</div>
              <div className="directory-view"><div className="directory-heading"><div className="directory-heading-top"><h3>Directory contents</h3><div className="directory-actions">{approvedWriteAccess && <><button className="directory-up" onClick={() => void createTextFile()}>+ File</button><button className="directory-up" onClick={() => void createSubfolder()}>+ Folder</button></>}{canNavigateUp && <button className="directory-up" onClick={() => void openFolder(parentPath)}>↑ Up</button>}</div></div><span>{currentPath || 'Select an approved folder'}</span></div>{busy ? <p className="muted-copy">Loading…</p> : entries.length === 0 ? <p className="muted-copy">Choose an approved root to browse its files and folders.</p> : <div className="entry-list">{entries.map((entry) => <button className="entry-row" key={entry.path} onClick={() => void (entry.is_dir ? openFolder(entry.path) : openFile(entry.path))}><span>{entry.is_dir ? '▰' : '▤'}</span><span>{entry.name}</span><small>{entry.is_dir ? 'Open →' : 'Preview'}</small></button>)}</div>}</div></div>
            {preview && <div className="file-preview"><div className="preview-heading"><strong>{approvedWriteAccess ? 'Text editor' : 'Text preview'}</strong><span>{preview.path}</span>{approvedWriteAccess && <><button disabled={!previewDirty} onClick={() => void savePreview()}>Save</button><button onClick={() => void copyPreview()}>Copy</button><button onClick={() => void renamePreview()}>Rename</button><button className="danger-action" onClick={() => void deletePreview()}>Delete</button></>}<button onClick={() => { setPreview(null); setPreviewDirty(false); }}>Close</button></div>{approvedWriteAccess ? <textarea className="file-editor" value={preview.content} onChange={(event) => { setPreview({ ...preview, content: event.target.value }); setPreviewDirty(true); }} spellCheck={false} /> : <pre>{preview.content}</pre>}</div>}
            <p className="hint-copy">Symbolic links are hidden. Read access is separate from write access. Writable folders support text editing, new files and folders, rename, and confirmed file deletion. Text reads and writes are limited to 1 MiB.</p>
          </div> : active === 'Processes' ? <div className="workspace-panel processes-panel">
            <div className="workspace-toolbar"><div><h2>Running processes</h2><p>Process metadata for your Linux user. Command arguments are not collected.</p></div><button className="primary-button" disabled={processBusy} onClick={() => void refreshProcesses()}>{processBusy ? 'Refreshing…' : 'Refresh'}</button></div>
            {error && <p className="error-message" role="alert">{error}</p>}
            {processSnapshot && <><div className="process-summary"><span>{processSnapshot.processes.length} processes shown</span><span>Current user only · Protected desktop and login processes cannot be stopped</span></div><div className="process-table-wrap"><table className="process-table"><thead><tr><th>PID</th><th>Process</th><th>Parent</th><th>State</th><th>Memory</th><th>Action</th></tr></thead><tbody>{processSnapshot.processes.map((process) => <tr key={process.pid}><td>{process.pid}</td><td>{process.name}</td><td>{process.parent_pid}</td><td>{process.state}</td><td>{formatMemory(process.resident_memory_bytes)}</td><td><button className="process-stop" disabled={process.protected} title={process.protection_reason ?? 'Stop this process'} onClick={() => void terminateProcess(process.pid, process.name)}>{process.protected ? 'Protected' : 'Stop'}</button></td></tr>)}</tbody></table></div>{processSnapshot.truncated && <p className="hint-copy">The list is capped at 500 processes. Some entries are not shown.</p>}</>}
            {!processSnapshot && !error && <p className="muted-copy">{processBusy ? 'Reading process information…' : 'No process information loaded.'}</p>}
            <p className="hint-copy">This feature reads Linux process metadata from /proc. It does not read command-line arguments, environment variables, or file contents. Stop sends SIGTERM only to eligible current-user processes. Desktop session, login infrastructure, this app, and its parent-session processes are protected.</p>
          </div> : active === 'Activity' ? <div className="workspace-panel activity-panel">
            <div className="workspace-toolbar"><div><h2>Recent activity</h2><p>Local audit history for desktop and MCP actions. File contents and command arguments are not recorded.</p></div><button className="primary-button" disabled={activityBusy} onClick={() => void refreshActivity()}>{activityBusy ? 'Refreshing…' : 'Refresh'}</button></div>
            {error && <p className="error-message" role="alert">{error}</p>}
            {activity.length === 0 ? <p className="muted-copy">{activityBusy ? 'Loading activity…' : 'No activity recorded yet.'}</p> : <div className="process-table-wrap"><table className="process-table"><thead><tr><th>Time</th><th>Source</th><th>Action</th><th>Target</th><th>Outcome</th></tr></thead><tbody>{activity.map((event, index) => <tr key={index}><td>{new Date(event.timestamp_unix_seconds * 1000).toLocaleString()}</td><td>{event.source}</td><td>{event.action}</td><td>{event.target ?? '—'}</td><td>{event.outcome}</td></tr>)}</tbody></table></div>}
            <p className="hint-copy">The log is stored locally in the application's data directory and rotates after reaching approximately 1 MiB. It records action metadata, not file contents or command arguments.</p>
          </div> : active === 'MCP server' ? <div className="workspace-panel mcp-panel">
            <div className="workspace-toolbar"><div><h2>Local MCP server</h2><p>Compatible AI clients launch this local process over standard input/output. No network port is opened.</p></div><span className="terminal-safety">STDIO · LOCAL</span></div>
            {error && <p className="error-message" role="alert">{error}</p>}
            <div className="mcp-status-grid">
              <article><span>Server binary</span><strong>{mcpInfo ? (mcpInfo.binary_exists ? 'Ready' : 'Not built') : 'Checking…'}</strong></article>
              <article><span>Transport</span><strong>STDIO</strong></article>
              <article><span>Network listener</span><strong>Disabled</strong></article>
              <article><span>Available tools</span><strong>{mcpInfo?.tools.length ?? '—'}</strong></article>
            </div>
            <div className="command-access-card">
              <div><h3>MCP command execution</h3><p>Runs one executable directly, without a shell, inside an approved working folder. It still runs with your normal Linux account permissions and is not sandboxed.</p></div>
              <button className={commandExecutionEnabled ? 'command-disable' : 'command-enable'} onClick={() => void toggleCommandExecution()}>{commandExecutionEnabled ? 'Disable command access' : 'Enable command access'}</button>
              <span className={commandExecutionEnabled ? 'command-state enabled' : 'command-state'}>{commandExecutionEnabled ? 'ENABLED' : 'DISABLED BY DEFAULT'}</span>
            </div>
            <div className="mcp-tools-list"><h3>Available MCP tools</h3>{(mcpInfo?.tools ?? ['list_approved_folders', 'list_directory', 'read_text_file']).map((tool) => <div key={tool}><code>{tool}</code><span>Available to connected clients</span></div>)}</div>
            <div className="mcp-config-section"><div><h3>Client configuration</h3><p>For MCP clients that accept the <code>mcpServers</code> JSON format, add this server entry to that client's configuration.</p></div>
              <pre className="mcp-config">{mcpInfo ? JSON.stringify({ mcpServers: { 'sams-desktop-commander': { command: mcpInfo.command, args: [] } } }, null, 2) : 'Loading executable path…'}</pre>
            </div>
            {mcpInfo && !mcpInfo.binary_exists && <p className="hint-copy">Build the local server from <code>app/src-tauri</code> with <code>CARGO_BUILD_JOBS=1 cargo build --no-default-features --features mcp --bin sam-desktop-mcp --locked</code>, then reopen this page.</p>}
            <p className="hint-copy">The MCP process reloads folder permissions before each tool call. File writes require a separate writable-folder approval. File deletion, directory creation, and moves require explicit confirmation fields. The run_command MCP tool is disabled until you enable it here. Each call also requires confirmation, runs without shell interpretation, has a 30-second timeout and 256 KiB output caps. It still runs with your normal Linux account permissions; it is not a sandbox.</p>
          </div> : active === 'Terminal' ? <div className="workspace-panel terminal-panel">
            <div className="workspace-toolbar"><div><h2>Interactive local terminal</h2><p>A real shell running as your desktop user, with keyboard input, terminal output, and resizing.</p></div><span className="terminal-safety">LOCAL PTY</span></div>
            {error && <p className="error-message" role="alert">{error}</p>}
            {folders.length === 0 ? <p className="muted-copy">Approve a folder in File access before starting a terminal.</p> : <>
              <div className="terminal-controls"><label>Initial working folder<select value={effectiveTerminalCwd} onChange={(e) => setTerminalCwd(e.target.value)}>{folders.map((folder) => <option key={folder} value={folder}>{folder}</option>)}</select></label></div>
              <InteractiveTerminal key={effectiveTerminalCwd} cwd={effectiveTerminalCwd} reportError={reportTerminalError} />
            </>}
            <p className="hint-copy">This shell is not a command allowlist. It runs with your normal Linux account permissions; the selected folder sets its starting directory but does not sandbox commands to that folder. Do not run commands you do not trust.</p>
                    </div> : <div className="empty-state"><div className="empty-icon">{sections.find((item) => item.label === active)?.icon}</div><h2>{active}</h2><p>This capability will be implemented after the permission and filesystem layers are verified.</p><button className="primary-button" onClick={() => setActive('Overview')}>Back to overview <span>↗</span></button></div>}
          <footer className="page-footer"><span>Sam's Desktop Commander</span><span>LOCAL-FIRST · PERMISSIONED · OPEN ARCHITECTURE</span></footer>
        </section>
      </main>
    </div>
  );
}
export default App;
