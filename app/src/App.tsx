import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import './App.css';

type Section = 'Overview' | 'File access' | 'Terminal' | 'MCP server' | 'Permissions' | 'Activity';
type Entry = { name: string; path: string; is_dir: boolean };
type CommandRun = { command: string; cwd: string; output: string };
type McpInfo = { command: string; binary_exists: boolean; transport: string; tools: string[] };
const sections: { label: Section; icon: string }[] = [
  { label: 'Overview', icon: '◫' }, { label: 'File access', icon: '▤' },
  { label: 'Terminal', icon: '>_' }, { label: 'MCP server', icon: '⌘' },
  { label: 'Permissions', icon: '◇' }, { label: 'Activity', icon: '◷' },
];

function App() {
  const [active, setActive] = useState<Section>('Overview');
  const [folders, setFolders] = useState<string[]>([]);
  const [currentPath, setCurrentPath] = useState('');
  const [entries, setEntries] = useState<Entry[]>([]);
  const [preview, setPreview] = useState<{ path: string; content: string } | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [terminalCwd, setTerminalCwd] = useState('');
  const [terminalCommand, setTerminalCommand] = useState('git status --short --branch');
  const [terminalOutput, setTerminalOutput] = useState<CommandRun[]>([]);
  const [terminalBusy, setTerminalBusy] = useState(false);
  const [mcpInfo, setMcpInfo] = useState<McpInfo | null>(null);

  useEffect(() => {
    let cancelled = false;
    void invoke<string[]>('approved_folders')
      .then((approved) => { if (!cancelled) setFolders(approved); })
      .catch((e) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, []);
  useEffect(() => {
    if (active !== 'MCP server') return;
    let cancelled = false;
    void invoke<McpInfo>('mcp_server_info')
      .then((info) => { if (!cancelled) setMcpInfo(info); })
      .catch((e) => { if (!cancelled) setError(String(e)); });
    return () => { cancelled = true; };
  }, [active]);
  const effectiveTerminalCwd = folders.includes(terminalCwd) ? terminalCwd : (folders[0] ?? '');

  const addFolder = async () => {
    setError('');
    const chosen = await open({ directory: true, multiple: false, title: 'Approve a folder for read-only access' });
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
    } catch (e) { setError(String(e)); setPreview(null); }
    finally { setBusy(false); }
  };

  const executeTerminalCommand = async () => {
    if (!effectiveTerminalCwd) { setError('Approve a folder before using the command runner.'); return; }
    setError(''); setTerminalBusy(true);
    try {
      const output = await invoke<string>('run_safe_command', { command: terminalCommand, cwd: effectiveTerminalCwd });
      setTerminalOutput((previous) => [...previous, { command: terminalCommand, cwd: effectiveTerminalCwd, output }].slice(-20));
    } catch (e) { setError(String(e)); }
    finally { setTerminalBusy(false); }
  };

  const revokeFolder = async (path: string) => {
    setError('');
    try {
      setFolders(await invoke<string[]>('revoke_folder', { path }));
      if (currentPath === path || currentPath.startsWith(`${path}/`)) { setCurrentPath(''); setEntries([]); setPreview(null); }
    } catch (e) { setError(String(e)); }
  };

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand"><div className="brand-mark">S</div><div><strong>Sam's Desktop</strong><span>COMMANDER</span></div></div>
        <div className="nav-label">WORKSPACE</div>
        <nav aria-label="Main navigation">{sections.map((section) => <button key={section.label} className={`nav-item ${active === section.label ? 'selected' : ''}`} onClick={() => setActive(section.label)}><span className="nav-icon">{section.icon}</span>{section.label}</button>)}</nav>
        <div className="sidebar-bottom"><span className="status-dot" /> Local machine <span className="version">v0.1.0</span></div>
      </aside>
      <main className="main-area">
        <header className="topbar"><div className="breadcrumb">Workspace <span>/</span> {active}</div><div className="local-pill"><span className="status-dot" /> LOCAL ONLY</div></header>
        <section className="page-content">
          <div className="page-heading"><div><p className="eyebrow">PERSONAL WORKSPACE</p><h1>{active}</h1><p className="subheading">Your computer, connected on your terms.</p></div><div className="date-chip">LINUX MINT <span>•</span> DESKTOP</div></div>
          {active === 'Overview' ? <>
            <div className="welcome-card"><div><div className="welcome-kicker">YOUR LOCAL CONTROL CENTER</div><h2>Welcome to your workspace.</h2><p>One place to manage local tools, access, and AI connections. You are in control of what gets permission.</p><div className="welcome-actions"><button className="primary-button" onClick={() => setActive('File access')}>Manage folders <span>↗</span></button><button className="text-button" onClick={() => setActive('MCP server')}>Configure MCP server</button></div></div><div className="orbit-art"><div className="orbit orbit-one"/><div className="orbit orbit-two"/><div className="orbit-core">S</div><span className="orbit-node node-one"/><span className="orbit-node node-two"/><span className="orbit-node node-three"/></div></div>
            <div className="section-title"><h3>System overview</h3><span>LOCAL STATUS</span></div>
            <div className="stats-grid"><article className="stat-card"><div className="stat-top"><span>Application</span><span className="mini-icon">▣</span></div><div className="stat-value">Running <i className="status-dot"/></div><p>Desktop shell initialized</p></article><article className="stat-card"><div className="stat-top"><span>MCP server</span><span className="mini-icon">⌘</span></div><div className="stat-value">Client-launched</div><p>STDIO process starts when an AI client connects</p></article><article className="stat-card"><div className="stat-top"><span>Approved folders</span><span className="mini-icon">▤</span></div><div className="stat-value">{folders.length} <small>folders</small></div><p>Read-only access currently</p></article><article className="stat-card"><div className="stat-top"><span>Remote access</span><span className="mini-icon">◇</span></div><div className="stat-value">Disabled</div><p>This application stays local</p></article></div>
            <div className="bottom-grid"><article className="panel"><div className="panel-heading"><h3>Getting started</h3><span>01 / 04</span></div><div className="step-row"><span className="step-number">01</span><div><strong>Set your access boundaries</strong><p>Choose which folders the application can use.</p></div><button onClick={() => setActive('File access')}>→</button></div><div className="step-row"><span className="step-number">02</span><div><strong>Configure the MCP server</strong><p>Expose approved tools to a compatible AI client.</p></div><button onClick={() => setActive('MCP server')}>→</button></div><div className="step-row"><span className="step-number">03</span><div><strong>Review activity</strong><p>Inspect actions and permission decisions.</p></div><button onClick={() => setActive('Activity')}>→</button></div></article><article className="panel security-panel"><div className="panel-heading"><h3>Security posture</h3><span className="safe-label">DEFAULT SAFE</span></div><div className="security-symbol">✓</div><h4>Local-first by design</h4><p>No remote listener is enabled. Folder listing is limited to explicitly approved roots.</p><div className="security-rule"><span>Network exposure</span><strong>None</strong></div><div className="security-rule"><span>Folder access</span><strong>Read-only</strong></div></article></div>
          </> : active === 'File access' ? <div className="workspace-panel">
            <div className="workspace-toolbar"><div><h2>Approved folders</h2><p>Only folders you approve are visible. Current operations are read-only.</p></div><button className="primary-button" disabled={busy} onClick={() => void addFolder()}>+ Approve folder</button></div>
            {error && <p className="error-message" role="alert">{error}</p>}
            <div className="folder-layout"><div className="folder-list"><h3>Access roots <span>{folders.length}</span></h3>{folders.length === 0 ? <p className="muted-copy">No folders approved yet.</p> : folders.map((folder) => <div className="folder-row" key={folder}><button className="folder-select" onClick={() => void openFolder(folder)}>▰ <span>{folder}</span></button><button className="revoke-button" onClick={() => void revokeFolder(folder)}>Revoke</button></div>)}</div>
              <div className="directory-view"><div className="directory-heading"><h3>Directory contents</h3><span>{currentPath || 'Select an approved folder'}</span></div>{busy ? <p className="muted-copy">Loading…</p> : entries.length === 0 ? <p className="muted-copy">Choose an approved root to browse its files and folders.</p> : <div className="entry-list">{entries.map((entry) => <button className="entry-row" key={entry.path} onClick={() => void (entry.is_dir ? openFolder(entry.path) : openFile(entry.path))}><span>{entry.is_dir ? '▰' : '▤'}</span><span>{entry.name}</span><small>{entry.is_dir ? 'Open →' : 'Preview'}</small></button>)}</div>}</div></div>
            {preview && <div className="file-preview"><div className="preview-heading"><strong>Text preview</strong><span>{preview.path}</span><button onClick={() => setPreview(null)}>Close</button></div><pre>{preview.content}</pre></div>}
            <p className="hint-copy">Symbolic links are hidden. Folder approvals are saved locally and revalidated when the app starts. Text previews are limited to 1 MiB.</p>
          </div> : active === 'MCP server' ? <div className="workspace-panel mcp-panel">
            <div className="workspace-toolbar"><div><h2>Local MCP server</h2><p>Compatible AI clients launch this local process over standard input/output. No network port is opened.</p></div><span className="terminal-safety">STDIO · LOCAL</span></div>
            {error && <p className="error-message" role="alert">{error}</p>}
            <div className="mcp-status-grid">
              <article><span>Server binary</span><strong>{mcpInfo ? (mcpInfo.binary_exists ? 'Ready' : 'Not built') : 'Checking…'}</strong></article>
              <article><span>Transport</span><strong>STDIO</strong></article>
              <article><span>Network listener</span><strong>Disabled</strong></article>
              <article><span>Available tools</span><strong>{mcpInfo?.tools.length ?? '—'}</strong></article>
            </div>
            <div className="mcp-tools-list"><h3>Read-only tools</h3>{(mcpInfo?.tools ?? ['list_approved_folders', 'list_directory', 'read_text_file']).map((tool) => <div key={tool}><code>{tool}</code><span>Available to connected clients</span></div>)}</div>
            <div className="mcp-config-section"><div><h3>Client configuration</h3><p>For MCP clients that accept the <code>mcpServers</code> JSON format, add this server entry to that client's configuration.</p></div>
              <pre className="mcp-config">{mcpInfo ? JSON.stringify({ mcpServers: { 'sams-desktop-commander': { command: mcpInfo.command, args: [] } } }, null, 2) : 'Loading executable path…'}</pre>
            </div>
            {mcpInfo && !mcpInfo.binary_exists && <p className="hint-copy">Build the local server from <code>app/src-tauri</code> with <code>CARGO_BUILD_JOBS=1 cargo build --no-default-features --features mcp --bin sam-desktop-mcp --locked</code>, then reopen this page.</p>}
            <p className="hint-copy">The MCP process reads the same saved folder approvals as this app and reloads them before each tool call. Revoking a folder therefore blocks the next request. This first version does not expose file writes, deletion, arbitrary terminal commands, or remote HTTP.</p>
          </div> : active === 'Terminal' ? <div className="workspace-panel terminal-panel">
            <div className="workspace-toolbar"><div><h2>Local command runner</h2><p>Run a small set of read-only diagnostic and Git commands inside an approved folder.</p></div><span className="terminal-safety">RESTRICTED COMMANDS</span></div>
            {error && <p className="error-message" role="alert">{error}</p>}
            {folders.length === 0 ? <p className="muted-copy">Approve a folder in File access before running commands.</p> : <>
              <div className="terminal-controls"><label>Working folder<select value={effectiveTerminalCwd} onChange={(e) => setTerminalCwd(e.target.value)}>{folders.map((folder) => <option key={folder} value={folder}>{folder}</option>)}</select></label>
              <label>Command<select value={terminalCommand} onChange={(e) => setTerminalCommand(e.target.value)}>{['pwd','git status --short --branch','git log -5 --oneline','git diff --stat','git branch --show-current','git rev-parse --show-toplevel','rustc --version','cargo --version','node --version','npm --version','python3 --version','free -h','df -h','uname -a'].map((item) => <option key={item} value={item}>{item}</option>)}</select></label>
              <button className="primary-button" disabled={terminalBusy || !effectiveTerminalCwd} onClick={() => void executeTerminalCommand()}>{terminalBusy ? 'Running…' : 'Run command'} <span>↗</span></button></div>
            </>}
            <div className="terminal-output"><div className="terminal-output-head"><span>OUTPUT</span><button onClick={() => setTerminalOutput([])} disabled={terminalOutput.length === 0}>Clear history</button></div>
              {terminalOutput.length === 0 ? <p className="terminal-empty">Command output will appear here.</p> : terminalOutput.map((run, index) => <article className="terminal-run" key={`${run.command}-${index}`}><div className="terminal-run-meta"><code>{run.command}</code><span>{run.cwd}</span></div><pre>{run.output}</pre></article>)}
            </div>
            <p className="hint-copy">Safety boundary: commands are selected from a fixed allowlist, no shell is invoked, and the working directory must remain inside an approved folder. File-changing commands are not enabled yet.</p>
          </div> : <div className="empty-state"><div className="empty-icon">{sections.find((item) => item.label === active)?.icon}</div><h2>{active}</h2><p>This capability will be implemented after the permission and filesystem layers are verified.</p><button className="primary-button" onClick={() => setActive('Overview')}>Back to overview <span>↗</span></button></div>}
          <footer className="page-footer"><span>Sam's Desktop Commander</span><span>LOCAL-FIRST · PERMISSIONED · OPEN ARCHITECTURE</span></footer>
        </section>
      </main>
    </div>
  );
}
export default App;
