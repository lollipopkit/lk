import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import {
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
  RevealOutputChannelOn,
  State as ClientState,
} from 'vscode-languageclient/node';
import type { Middleware } from 'vscode-languageclient/node';
import { execFile } from 'child_process';

let client: LanguageClient;
let statusBarItem: vscode.StatusBarItem;
let isManuallyDisabled = false;
let checkInFlight = 0;
let checkIdleTimer: NodeJS.Timeout | undefined;
let checkStartTimer: NodeJS.Timeout | undefined;

// Lightweight performance tracing (extension-side)
let perfTraceSteps = false;
let perfThresholdMs = 0;

function perfLog(label: string, ms: number) {
  if (!perfTraceSteps) return;
  if (ms < perfThresholdMs) return;
  try {
    console.log(`[LKR Perf] ${label} took ${ms.toFixed(1)} ms`);
  } catch {
    // ignore logging failures
  }
}

function withTiming<T>(label: string, fn: () => T): T {
  const start = Date.now();
  try {
    const result = fn();
    const maybePromise = result as any;
    if (maybePromise && typeof maybePromise.then === 'function') {
      return (maybePromise.finally(() => perfLog(label, Date.now() - start)) as unknown) as T;
    }
    perfLog(label, Date.now() - start);
    return result;
  } catch (e) {
    perfLog(`${label} (error)`, Date.now() - start);
    throw e;
  }
}

// Simple LRU cache for perf-sensitive middleware
class LRU<K, V> {
  private map = new Map<K, V>();
  constructor(public capacity: number) {}
  get(key: K): V | undefined {
    if (!this.map.has(key)) return undefined;
    const val = this.map.get(key)!;
    this.map.delete(key);
    this.map.set(key, val);
    return val;
  }
  set(key: K, value: V) {
    if (this.map.has(key)) this.map.delete(key);
    this.map.set(key, value);
    if (this.map.size > this.capacity) {
      const iter = this.map.keys().next();
      if (!iter.done) {
        this.map.delete(iter.value as K);
      }
    }
  }
  has(key: K): boolean { return this.map.has(key); }
  clear() { this.map.clear(); }
  setCapacity(n: number) {
    this.capacity = Math.max(1, Math.floor(n || 1));
    while (this.map.size > this.capacity) {
      const iter = this.map.keys().next();
      if (iter.done) break;
      this.map.delete(iter.value as K);
    }
  }
}

// Runtime settings snapshot (kept in sync with workspace configuration)
const runtime = {
  semanticTokensEnabled: true,
  semanticTokensThrottleMs: 120,
  // auto | rangeOnly | fullOnly
  semanticTokensMode: 'auto' as 'auto' | 'rangeOnly' | 'fullOnly',
  autoRangeAtLines: 800,
  inlayHintsEnabled: true,
  inlayHintsThrottleMs: 50,
  inlayHintsShowParameters: true,
  inlayHintsShowTypes: true,
  checkingDelayMs: 120,
};

export function activate(context: vscode.ExtensionContext) {
  console.log('LKR extension is now active');

  // Create status bar item
  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  statusBarItem.text = '$(sync~spin) LKR LSP: Starting...';
  statusBarItem.tooltip = 'LKR Language Server is starting';
  statusBarItem.command = 'lkr.showStatusBarMenu';
  statusBarItem.show();
  context.subscriptions.push(statusBarItem);

  // Register commands
  const startCommand = vscode.commands.registerCommand('lkr.startServer', async () => {
    if (!client) {
      vscode.window.showErrorMessage('LKR Language Server client not initialized');
      return;
    }
    try {
      updateStatusBar('starting');
      await withTiming('command.startServer', () => client.start());
      vscode.window.showInformationMessage('LKR Language Server started');
    } catch (e: any) {
      vscode.window.showErrorMessage('Failed to start LKR Language Server: ' + (e?.message || e));
    }
  });

  const restartCommand = vscode.commands.registerCommand('lkr.restartServer', async () => {
    if (client) {
      console.log('Restarting LKR Language Server...');
      updateStatusBar('starting');
      await client.stop();
      await withTiming('command.restartServer.start', () => client.start());
      vscode.window.showInformationMessage('LKR Language Server restarted');
    }
  });

  const statusBarMenuCommand = vscode.commands.registerCommand('lkr.showStatusBarMenu', async () => {
    const items: vscode.QuickPickItem[] = [];
    
    if (isManuallyDisabled) {
      items.push({
        label: '$(play) Enable LKR LSP',
        description: 'Start the language server',
        detail: 'Enable LKR Language Server'
      });
    } else {
      items.push({
        label: '$(sync) Restart LKR LSP',
        description: 'Restart the language server',
        detail: 'Restart LKR Language Server'
      });
      
      items.push({
        label: '$(circle-slash) Disable LKR LSP',
        description: 'Temporarily disable (memory state)',
        detail: 'Disable LKR Language Server temporarily'
      });

      // Inline menu toggles for inlay hints
      items.push({
        label: `${runtime.inlayHintsEnabled ? '$(eye)' : '$(eye-closed)'} Toggle Inlay Hints`,
        description: runtime.inlayHintsEnabled ? 'Disable inline hints' : 'Enable inline hints',
        detail: `Parameters: ${runtime.inlayHintsShowParameters ? 'on' : 'off'}, Types: ${runtime.inlayHintsShowTypes ? 'on' : 'off'}`,
      });
      items.push({
        label: `${runtime.inlayHintsShowParameters ? '$(check)' : '$(circle-slash)'} Parameter Hints`,
        description: 'Show argument names in calls',
        detail: 'Editor inlay hints: parameters',
      });
      items.push({
        label: `${runtime.inlayHintsShowTypes ? '$(check)' : '$(circle-slash)'} Type Hints`,
        description: 'Show inferred types for declarations',
        detail: 'Editor inlay hints: types',
      });
    }
    
    const selected = await vscode.window.showQuickPick(items, {
      placeHolder: 'LKR Language Server Actions',
      title: 'LKR Language Server'
    });
    
    if (!selected) return;
    
    if (selected.label.includes('Enable')) {
      isManuallyDisabled = false;
      await vscode.commands.executeCommand('lkr.startServer');
    } else if (selected.label.includes('Restart')) {
      await vscode.commands.executeCommand('lkr.restartServer');
    } else if (selected.label.includes('Disable')) {
      isManuallyDisabled = true;
      if (client) {
        await client.stop();
      }
      updateStatusBar('disabled');
      vscode.window.showInformationMessage('LKR Language Server disabled temporarily');
    } else if (selected.label.includes('Toggle Inlay Hints')) {
      runtime.inlayHintsEnabled = !runtime.inlayHintsEnabled;
      await vscode.workspace.getConfiguration('lkr.lsp').update('inlayHints.enabled', runtime.inlayHintsEnabled, vscode.ConfigurationTarget.Workspace);
      vscode.window.showInformationMessage(`LKR Inlay Hints ${runtime.inlayHintsEnabled ? 'enabled' : 'disabled'}`);
      // Trigger refresh
      await vscode.commands.executeCommand('editor.action.inlineHints.refresh');
    } else if (selected.label.includes('Parameter Hints')) {
      runtime.inlayHintsShowParameters = !runtime.inlayHintsShowParameters;
      await vscode.workspace.getConfiguration('lkr.lsp').update('inlayHints.parameters.enabled', runtime.inlayHintsShowParameters, vscode.ConfigurationTarget.Workspace);
      vscode.window.showInformationMessage(`LKR Parameter Hints ${runtime.inlayHintsShowParameters ? 'enabled' : 'disabled'}`);
      await vscode.commands.executeCommand('editor.action.inlineHints.refresh');
    } else if (selected.label.includes('Type Hints')) {
      runtime.inlayHintsShowTypes = !runtime.inlayHintsShowTypes;
      await vscode.workspace.getConfiguration('lkr.lsp').update('inlayHints.types.enabled', runtime.inlayHintsShowTypes, vscode.ConfigurationTarget.Workspace);
      vscode.window.showInformationMessage(`LKR Type Hints ${runtime.inlayHintsShowTypes ? 'enabled' : 'disabled'}`);
      await vscode.commands.executeCommand('editor.action.inlineHints.refresh');
    }
  });

  context.subscriptions.push(startCommand, restartCommand, statusBarMenuCommand);

  // Analyze current file via lkr-lsp --analyze (uses relative, sanitized path)
  const analyzeCommand = vscode.commands.registerCommand('lkr.analyzeCurrentFile', async () => {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.document.languageId !== 'lkr') {
      vscode.window.showWarningMessage('Open a LKR file to analyze.');
      return;
    }
    const ws = vscode.workspace.workspaceFolders?.[0];
    if (!ws) {
      vscode.window.showWarningMessage('Open a workspace folder to run analysis.');
      return;
    }

    const abs = editor.document.uri.fsPath;
    const root = ws.uri.fsPath;
    let rel = path.relative(root, abs);
    // Normalize to POSIX-like separators for CLI and guard against .. or absolute
    rel = rel.split(path.sep).join('/');
    if (!rel || rel.startsWith('..') || path.isAbsolute(rel) || rel.includes('..')) {
      vscode.window.showErrorMessage('Refusing to analyze: file must be inside the workspace and use a safe relative path.');
      return;
    }

    const pick = await vscode.window.showQuickPick([
      { label: 'Full JSON', description: 'Show full analysis output' },
      { label: 'Errors Only', description: 'List only errors' }
    ], { title: 'LKR Analyze Current File' });
    if (!pick) return;

    const serverPath = getServerPath();
    if (!serverPath) {
      vscode.window.showErrorMessage('LKR LSP server binary not found. Build the project or configure lkr.lsp.serverPath.');
      return;
    }

    const args = ['--analyze'];
    if (pick.label.startsWith('Errors')) args.push('--errors-only');
    args.push(rel);

    const out = vscode.window.createOutputChannel('LKR Analysis');
    out.clear();
    out.show(true);
    out.appendLine(`Running: ${serverPath} ${args.join(' ')}`);
    const _start = Date.now();
    execFile(serverPath, args, { cwd: root }, (err, stdout, stderr) => {
      if (err) {
        out.appendLine('--- Error ---');
        out.appendLine(String(err.message || err));
      }
      if (stderr && stderr.trim().length) {
        out.appendLine('--- Stderr ---');
        out.appendLine(stderr);
      }
      if (stdout && stdout.trim().length) {
        out.appendLine('--- Output ---');
        out.appendLine(stdout);
      }
      perfLog('command.analyzeCurrentFile', Date.now() - _start);
    });
  });
  context.subscriptions.push(analyzeCommand);

  // Check if LSP is enabled
  const config = vscode.workspace.getConfiguration('lkr.lsp');
  // Initialize perf tracing settings early
  perfTraceSteps = config.get<boolean>('performance.traceSteps', false);
  perfThresholdMs = Math.max(0, Number(config.get<number>('performance.traceThresholdMs', 0)) || 0);
  const lspEnabled = config.get<boolean>('enabled', true);
  const autoStart = config.get<boolean>('autoStart', true);
  // Load runtime settings from configuration
  runtime.semanticTokensEnabled = config.get<boolean>('semanticTokens.enabled', true);
  runtime.semanticTokensThrottleMs = Math.max(0, Number(config.get<number>('semanticTokens.throttleMs', 120)) || 0);
  runtime.semanticTokensMode = (config.get<string>('semanticTokens.mode', 'auto') as any) || 'auto';
  runtime.autoRangeAtLines = Math.max(1, Number(config.get<number>('semanticTokens.autoRangeAtLines', 800)) || 800);
  runtime.inlayHintsEnabled = config.get<boolean>('inlayHints.enabled', true);
  runtime.inlayHintsThrottleMs = Math.max(0, Number(config.get<number>('inlayHints.throttleMs', 50)) || 0);
  runtime.inlayHintsShowParameters = config.get<boolean>('inlayHints.parameters.enabled', true);
  runtime.inlayHintsShowTypes = config.get<boolean>('inlayHints.types.enabled', true);
  runtime.checkingDelayMs = Math.max(0, Number(config.get<number>('ui.checkingDelayMs', 120)) || 120);
  
  if (!lspEnabled || isManuallyDisabled) {
    console.log('LKR LSP is disabled in configuration or manually disabled');
    updateStatusBar('disabled');
    return;
  }

  // Get the path to the LKR LSP server
  const customServerPath = config.get<string>('serverPath', '');
  const serverPath = withTiming('resolveServerPath', () => (customServerPath ? expandHome(customServerPath) : getServerPath()));

  console.log('Looking for LKR LSP server...');
  console.log('Server path resolved to:', serverPath ?? 'PATH: lkr-lsp');

  // If the server path is not found, show an error and return
  if (!serverPath) {
    updateStatusBar('error', 'Server not found');
    vscode.window.showErrorMessage(
      'LKR LSP server not found. Please build the LKR project first or configure a custom server path.'
    );
    return;
  }

  const serverOptions: ServerOptions = {
    command: serverPath,
    transport: TransportKind.stdio
  };

  const traceLevel = config.get<string>('trace', 'off');
  const isVerbose = traceLevel === 'verbose';
  const outputChannelEnabled = config.get<boolean>('outputChannel.enabled', false);
  const semanticTokensEnabled = runtime.semanticTokensEnabled;
  const throttleMs = runtime.semanticTokensThrottleMs;

  // Lightweight, per-document throttle map (separate for tokens and hints)
  const lastTokenReqAt = new Map<string, number>();
  const lastInlayReqAt = new Map<string, number>();
  // Simple per-document concurrency gates to avoid piling up work while scrolling
  const tokenInFlight = new Set<string>();
  const inlayInFlight = new Set<string>();
  // Keyed in-flight promises to dedupe identical requests
  const dedupeTokenRange = new Map<string, Promise<any>>();
  const dedupeInlay = new Map<string, Promise<any>>();
  // Caches (LRU)
  const rangeTokenCacheLimit = Math.max(1, Number(config.get<number>('performance.rangeTokenCacheLimit', 64)) || 64);
  const inlayHintCacheLimit = Math.max(1, Number(config.get<number>('performance.inlayHintCacheLimit', 64)) || 64);
  let enableCaching = config.get<boolean>('performance.enableCaching', true);
  const tokensCache = new LRU<string, vscode.SemanticTokens | null | undefined>(rangeTokenCacheLimit);
  const inlayCache = new LRU<string, (vscode.InlayHint[] | null | undefined)>(inlayHintCacheLimit);
  let skipStaleResults = config.get<boolean>('performance.skipStaleResults', true);
  const settings = { semanticTokensEnabled, throttleMs };
  
  const middleware: Middleware = {
    // Surface diagnostics flow to toggle checking status when results arrive
    handleDiagnostics(uri, diagnostics, next) {
      try {
        // Diagnostics arrived: clear any pending checking indicator
        endChecking();
      } finally {
        next(uri, diagnostics);
      }
    },
    provideDocumentSemanticTokens(document, token, next) {
      if (!settings.semanticTokensEnabled) {
        if (isVerbose) console.log('Semantic tokens disabled (full)');
        return null;
      }
      // semantic tokens mode logic
      const mode = runtime.semanticTokensMode;
      if (mode === 'rangeOnly' || (mode === 'auto' && document.lineCount >= runtime.autoRangeAtLines)) {
        if (isVerbose) console.log('Skip full tokens due to mode');
        return null;
      }
      if (token?.isCancellationRequested) return null;
      const reqVersion = document.version;
      const key = document.uri.toString();
      if (tokenInFlight.has(key)) {
        perfLog('skip.semanticTokens(full).inflight', 0);
        return null;
      }
      beginChecking('semanticTokens(full)');
      if (settings.throttleMs > 0) {
        const now = Date.now();
        const last = lastTokenReqAt.get(key) || 0;
        if (now - last < settings.throttleMs) {
          if (isVerbose) console.log('Semantic tokens full throttled');
          endChecking();
          return null;
        }
        lastTokenReqAt.set(key, now);
      }
      // Cache key for full tokens (by version)
      const cacheKey = `${key}#v${reqVersion}#FULL`;
      if (enableCaching) {
        const cached = tokensCache.get(cacheKey);
        if (cached !== undefined) {
          endChecking();
          return cached as any;
        }
      }
      tokenInFlight.add(key);
      const result = withTiming('middleware.semanticTokens(full)', () => next(document, token));
      if (result && typeof (result as any).then === 'function') {
        return (result as Promise<any>)
          .then(res => {
            if (token?.isCancellationRequested) return null as any;
            if (skipStaleResults && document.version !== reqVersion) return null as any;
            if (enableCaching) tokensCache.set(cacheKey, res as any);
            return res;
          })
          .finally(() => { tokenInFlight.delete(key); endChecking(); });
      } else {
        try {
          if (enableCaching) tokensCache.set(cacheKey, result as any);
          return result as any;
        } finally {
          tokenInFlight.delete(key);
          endChecking();
        }
      }
    },
    provideDocumentRangeSemanticTokens(document, range, token, next) {
      if (!settings.semanticTokensEnabled) {
        if (isVerbose) console.log('Semantic tokens disabled (range)');
        return null;
      }
      // semantic tokens mode logic
      const mode = runtime.semanticTokensMode;
      if (mode === 'fullOnly' || (mode === 'auto' && document.lineCount < runtime.autoRangeAtLines)) {
        if (isVerbose) console.log('Skip range tokens due to mode');
        return null;
      }
      if (token?.isCancellationRequested) return null;
      const reqVersion = document.version;
      const key = document.uri.toString();
      if (tokenInFlight.has(key)) {
        perfLog('skip.semanticTokens(range).inflight', 0);
        return null;
      }
      beginChecking('semanticTokens(range)');
      if (settings.throttleMs > 0) {
        const now = Date.now();
        const last = lastTokenReqAt.get(key) || 0;
        if (now - last < settings.throttleMs) {
          if (isVerbose) console.log('Semantic tokens range throttled');
          endChecking();
          return null;
        }
        lastTokenReqAt.set(key, now);
      }
      const rKey = `${range.start.line}:${range.start.character}-${range.end.line}:${range.end.character}`;
      const cacheKey = `${key}#v${reqVersion}#R#${rKey}`;
      if (enableCaching) {
        const cached = tokensCache.get(cacheKey);
        if (cached !== undefined) {
          endChecking();
          return cached as any;
        }
      }
      if (dedupeTokenRange.has(cacheKey)) {
        return dedupeTokenRange.get(cacheKey)! as any;
      }
      tokenInFlight.add(key);
      const p = withTiming('middleware.semanticTokens(range)', () => next(document, range, token)) as Promise<any>;
      const wrapped = p.then(res => {
        if (token?.isCancellationRequested) return null as any;
        if (skipStaleResults && document.version !== reqVersion) return null as any;
        if (enableCaching) tokensCache.set(cacheKey, res as any);
        return res;
      }).finally(() => { tokenInFlight.delete(key); dedupeTokenRange.delete(cacheKey); endChecking(); });
      dedupeTokenRange.set(cacheKey, wrapped);
      return wrapped as any;
    },
    // Inlay hints: show checking spinner and support throttling + filtering
    provideInlayHints(document, range, token, next) {
      if (!runtime.inlayHintsEnabled) {
        return null;
      }
      if (token?.isCancellationRequested) return null;
      const reqVersion = document.version;
      const key = document.uri.toString();
      if (inlayInFlight.has(key)) {
        perfLog('skip.inlayHints.inflight', 0);
        return null;
      }
      beginChecking('inlayHints');
      if (runtime.inlayHintsThrottleMs > 0) {
        const now = Date.now();
        const last = lastInlayReqAt.get(key) || 0;
        if (now - last < runtime.inlayHintsThrottleMs) {
          endChecking();
          return null;
        }
        lastInlayReqAt.set(key, now);
      }
      const rKey = `${range.start.line}:${range.start.character}-${range.end.line}:${range.end.character}`;
      const settingsKey = `${Number(runtime.inlayHintsShowParameters)}:${Number(runtime.inlayHintsShowTypes)}`;
      const cacheKey = `${key}#v${reqVersion}#I#${rKey}#${settingsKey}`;
      if (enableCaching) {
        const cached = inlayCache.get(cacheKey);
        if (cached !== undefined) {
          endChecking();
          return cached as any;
        }
      }
      if (dedupeInlay.has(cacheKey)) {
        return dedupeInlay.get(cacheKey)! as any;
      }
      inlayInFlight.add(key);
      const res = withTiming('middleware.inlayHints', () => next(document, range, token));
      const filter = (hints: vscode.InlayHint[] | null | undefined) => {
        if (!hints) return hints;
        const wantParams = runtime.inlayHintsShowParameters;
        const wantTypes = runtime.inlayHintsShowTypes;
        return hints.filter(h => {
          const kind = (h.kind ?? vscode.InlayHintKind.Type);
          if (kind === vscode.InlayHintKind.Parameter) return wantParams;
          if (kind === vscode.InlayHintKind.Type) return wantTypes;
          return true;
        });
      };
      if (res && typeof (res as any).then === 'function') {
        const p = (res as Promise<vscode.InlayHint[] | null | undefined>)
          .then(v => {
            if (token?.isCancellationRequested) return null;
            if (skipStaleResults && document.version !== reqVersion) return null;
            const filtered = filter(v);
            if (enableCaching) inlayCache.set(cacheKey, filtered as any);
            return filtered;
          })
          .finally(() => { inlayInFlight.delete(key); dedupeInlay.delete(cacheKey); endChecking(); });
        dedupeInlay.set(cacheKey, p as any);
        return p as any;
      } else {
        try {
          const filtered = filter(res as any);
          if (enableCaching) inlayCache.set(cacheKey, filtered as any);
          return filtered;
        } finally {
          inlayInFlight.delete(key);
          endChecking();
        }
      }
    },
    // If the server emits WorkDone progress, reflect it in the status bar
    handleWorkDoneProgress(token, params, next) {
      try {
        if (params && (params as any).kind) {
          const kind = (params as any).kind as 'begin' | 'report' | 'end';
          if (kind === 'begin') {
            beginChecking('progress');
          } else if (kind === 'end') {
            endChecking();
          }
        }
      } finally {
        next(token, params);
      }
    }
  };

  // React to configuration changes
  context.subscriptions.push(vscode.workspace.onDidChangeConfiguration(e => {
    if (!e.affectsConfiguration('lkr.lsp')) return;
    const cfg = vscode.workspace.getConfiguration('lkr.lsp');
    runtime.semanticTokensEnabled = cfg.get<boolean>('semanticTokens.enabled', true);
    runtime.semanticTokensThrottleMs = Math.max(0, Number(cfg.get<number>('semanticTokens.throttleMs', 120)) || 0);
    runtime.semanticTokensMode = (cfg.get<string>('semanticTokens.mode', 'auto') as any) || 'auto';
    runtime.autoRangeAtLines = Math.max(1, Number(cfg.get<number>('semanticTokens.autoRangeAtLines', 800)) || 800);
    runtime.inlayHintsEnabled = cfg.get<boolean>('inlayHints.enabled', true);
    runtime.inlayHintsThrottleMs = Math.max(0, Number(cfg.get<number>('inlayHints.throttleMs', 50)) || 0);
    runtime.inlayHintsShowParameters = cfg.get<boolean>('inlayHints.parameters.enabled', true);
    runtime.inlayHintsShowTypes = cfg.get<boolean>('inlayHints.types.enabled', true);
    // Perf tracing settings
    perfTraceSteps = cfg.get<boolean>('performance.traceSteps', false);
    perfThresholdMs = Math.max(0, Number(cfg.get<number>('performance.traceThresholdMs', 0)) || 0);
    // Cache and stale handling settings
    enableCaching = cfg.get<boolean>('performance.enableCaching', true);
    skipStaleResults = cfg.get<boolean>('performance.skipStaleResults', true);
    const newTokenCap = Math.max(1, Number(cfg.get<number>('performance.rangeTokenCacheLimit', 64)) || 64);
    const newInlayCap = Math.max(1, Number(cfg.get<number>('performance.inlayHintCacheLimit', 64)) || 64);
    tokensCache.setCapacity(newTokenCap);
    inlayCache.setCapacity(newInlayCap);
    runtime.checkingDelayMs = Math.max(0, Number(cfg.get<number>('ui.checkingDelayMs', 120)) || 120);
    // Soft nudge so users see status change quickly
    nudgeChecking();
  }));

  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ scheme: 'file', language: 'lkr' }],
    synchronize: {
      configurationSection: 'lkr'
    },
    // Initialize options for semantic highlighting
    initializationOptions: {
      // Enable semantic highlighting
      semanticHighlighting: true,
      // Custom configuration for LKR
      lkr: {
        enableSemanticTokens: true
      }
    },
    // Never auto-reveal the output unless user explicitly opens it
    revealOutputChannelOn: RevealOutputChannelOn.Never,
    traceOutputChannel: traceLevel !== 'off' ? vscode.window.createOutputChannel('LKR Language Server Trace') : undefined,
    middleware
  };

  // Create and attach an output channel only when enabled/verbose
  if (outputChannelEnabled || isVerbose) {
    const outputChannel = vscode.window.createOutputChannel('LKR Language Server');
    clientOptions.outputChannel = outputChannel;
  }

  client = withTiming('createLanguageClient', () => new LanguageClient(
    'lkr',
    'LKR Language Server',
    serverOptions,
    clientOptions
  ));

  if (isVerbose) {
    console.log('Starting LKR Language Server...', serverPath);
  }
  
  // Add error handling for the client itself
  client.onDidChangeState((event) => {
    if (isVerbose) {
      console.log(`LSP client state change: ${event.oldState} -> ${event.newState}`);
    }
    
    // Update status bar based on state
    switch (event.newState) {
      case ClientState.Starting:
        updateStatusBar('starting');
        break;
      case ClientState.Running:
        updateStatusBar('running');
        break;
      case ClientState.Stopped:
        updateStatusBar('stopped');
        break;
    }
  });
  
  // Avoid extra semantic token logging to keep UI responsive

  // Start with a timeout and proper error handling
  const startPromise = autoStart ? withTiming('lsp.start', () => client.start()) : Promise.resolve();
  
  // Add a timeout to detect hanging
  const timeoutPromise = new Promise((_, reject) => {
    setTimeout(() => reject(new Error('LSP server start timeout after 10 seconds')), 10000);
  });
  
  Promise.race([startPromise, timeoutPromise])
    .then(() => {
      if (isVerbose && autoStart) {
        console.log('LKR Language Server started successfully');
        // Check if semantic highlighting is enabled
        const editorConfig = vscode.workspace.getConfiguration('editor');
        const semanticHighlighting = editorConfig.get('semanticHighlighting.enabled');
        console.log('Semantic highlighting enabled:', semanticHighlighting);
      }
      updateStatusBar('running');
    })
    .catch((error) => {
      console.error('Failed to start LKR Language Server:', error);
      console.error('Error details:', JSON.stringify(error, null, 2));
      vscode.window.showErrorMessage('Failed to start LKR Language Server: ' + error.message);
      updateStatusBar('error', 'Start failed');
      
      // Try to stop the client if it's in a bad state
      if (client) {
        client.stop().catch(stopError => {
          console.error('Error stopping client after failure:', stopError);
        });
      }
    });
  
  // Mark as checking when LKR documents change or save; diagnostics will clear it
  context.subscriptions.push(vscode.workspace.onDidChangeTextDocument(e => {
    if (e.document.languageId === 'lkr') {
      nudgeChecking();
    }
  }));
  context.subscriptions.push(vscode.workspace.onDidSaveTextDocument(doc => {
    if (doc.languageId === 'lkr') {
      nudgeChecking();
    }
  }));
  // React to configuration changes at runtime
  context.subscriptions.push(vscode.workspace.onDidChangeConfiguration(e => {
    if (e.affectsConfiguration('lkr.lsp.semanticTokens.enabled') || e.affectsConfiguration('lkr.lsp.semanticTokens.throttleMs')) {
      const cfg = vscode.workspace.getConfiguration('lkr.lsp');
      settings.semanticTokensEnabled = cfg.get<boolean>('semanticTokens.enabled', true);
      settings.throttleMs = Math.max(0, Number(cfg.get<number>('semanticTokens.throttleMs', 40)) || 0);
      if (isVerbose) console.log('Updated semantic tokens settings', settings);
    }
  }));
}

function getServerPath(): string | undefined {
  // Try to find the lkr-lsp executable in different locations
  const exe = process.platform === 'win32' ? '.exe' : '';
  const possiblePaths = [
    // When developing inside repo: extension is at vsc-ext/lsp/out
    // Look for cargo build outputs at repo root target/{debug,release}
    path.join(__dirname, '..', '..', '..', 'target', 'debug', `lkr-lsp${exe}`),
    path.join(__dirname, '..', '..', '..', 'target', 'release', `lkr-lsp${exe}`),
    // Fallbacks that may exist depending on packaging layout
    path.join(__dirname, '..', '..', 'target', 'debug', `lkr-lsp${exe}`),
    path.join(__dirname, '..', '..', 'target', 'release', `lkr-lsp${exe}`),
    // Common user install
    expandHome(`~/.cargo/bin/lkr-lsp${exe}`),
  ];

  // Reduce noisy logs unless verbose
  // console.log('Extension __dirname:', __dirname);
  // console.log('Searching for lkr-lsp binary in paths:');
  
  for (const possiblePath of possiblePaths) {
    if (!possiblePath) continue;
    try {
      if (fs.existsSync(possiblePath)) {
        // Test if the file is executable (skip X_OK on Windows)
        const mode = process.platform === 'win32'
          ? fs.constants.F_OK
          : (fs.constants.F_OK | fs.constants.X_OK);
        fs.accessSync(possiblePath, mode);
        return possiblePath;
      }
    } catch {
      // ignore
    }
  }

  // Fall back to PATH resolution by returning command name
  return process.platform === 'win32' ? 'lkr-lsp.exe' : 'lkr-lsp';
}

function expandHome(p: string): string {
  if (!p) return '';
  if (p.startsWith('~')) {
    const home = process.env.HOME || process.env.USERPROFILE || '';
    return path.join(home, p.slice(1));
  }
  return p;
}

function updateStatusBar(state: string, customMessage?: string) {
  if (!statusBarItem) {
    return;
  }
  
  switch (state) {
    case 'starting':
      statusBarItem.text = '$(sync~spin) LKR LSP: Starting...';
      statusBarItem.tooltip = 'LKR Language Server is starting';
      break;
    case 'checking':
      statusBarItem.text = '$(sync~spin) LKR LSP: Checking...';
      statusBarItem.tooltip = 'LKR Language Server is analyzing/validating';
      break;
    case 'running':
      statusBarItem.text = '$(check) LKR LSP: Running';
      statusBarItem.tooltip = 'LKR Language Server is running';
      break;
    case 'stopped':
      statusBarItem.text = '$(circle-slash) LKR LSP: Stopped';
      statusBarItem.tooltip = 'LKR Language Server is stopped';
      break;
    case 'error':
      statusBarItem.text = '$(error) LKR LSP: Error';
      statusBarItem.tooltip = customMessage ? `LKR Language Server error: ${customMessage}` : 'LKR Language Server error';
      break;
    case 'disabled':
      statusBarItem.text = '$(circle-slash) LKR LSP: Disabled';
      statusBarItem.tooltip = isManuallyDisabled ? 'LKR Language Server is temporarily disabled (click to enable)' : 'LKR Language Server is disabled in settings';
      break;
    default:
      statusBarItem.text = '$(question) LKR LSP: Unknown';
      statusBarItem.tooltip = 'LKR Language Server status unknown';
  }
}

function beginChecking(_reason?: string) {
  if (!statusBarItem || isManuallyDisabled) {
    return;
  }
  const wasIdle = checkInFlight === 0;
  checkInFlight++;
  if (wasIdle) {
    if (checkIdleTimer) {
      clearTimeout(checkIdleTimer);
      checkIdleTimer = undefined;
    }
    if (checkStartTimer) clearTimeout(checkStartTimer);
    const delay = Math.max(0, runtime.checkingDelayMs || 0);
    checkStartTimer = setTimeout(() => {
      if (checkInFlight > 0) updateStatusBar('checking');
    }, delay);
  }
}

function endChecking() {
  if (!statusBarItem) return;
  if (checkInFlight > 0) checkInFlight--;
  if (checkInFlight === 0) {
    if (checkStartTimer) { clearTimeout(checkStartTimer); checkStartTimer = undefined; }
    if (checkIdleTimer) clearTimeout(checkIdleTimer);
    // Small delay to avoid flicker if more work immediately follows
    checkIdleTimer = setTimeout(() => updateStatusBar('running'), 150);
  }
}

// UI-only nudge to show 'Checking…' without affecting the in-flight counter.
function nudgeChecking() {
  if (!statusBarItem || isManuallyDisabled) return;
  if (checkInFlight === 0) {
    if (checkIdleTimer) {
      clearTimeout(checkIdleTimer);
      checkIdleTimer = undefined;
    }
    updateStatusBar('checking');
  }
}

export function deactivate(): Thenable<void> | undefined {
  if (!client) {
    return undefined;
  }
  updateStatusBar('stopped');
  return client.stop();
}
