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
let outputChannel: vscode.OutputChannel;
let isManuallyDisabled = false;
let checkInFlight = 0;
let checkIdleTimer: NodeJS.Timeout | undefined;
let checkStartTimer: NodeJS.Timeout | undefined;
let nudgeTimer: NodeJS.Timeout | undefined;
let nudgeClearTimer: NodeJS.Timeout | undefined;

// Lightweight performance tracing (extension-side)
let perfTraceSteps = false;
let perfThresholdMs = 0;

function perfLog(label: string, ms: number) {
  if (!perfTraceSteps) return;
  if (ms < perfThresholdMs) return;
  try {
    log(`[LK Perf] ${label} took ${ms.toFixed(1)} ms`);
  } catch {
    // ignore logging failures
  }
}

function log(message: string) {
  const line = `${new Date().toISOString()} ${message}`;
  try {
    console.log(line);
    outputChannel?.appendLine(line);
  } catch {
    // ignore logging failures
  }
}

function logError(message: string, error?: unknown) {
  const details = error instanceof Error ? `${error.message}\n${error.stack ?? ''}` : String(error ?? '');
  log(details ? `${message}: ${details}` : message);
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

function trustHoverCommands(hover: vscode.Hover | null | undefined): vscode.Hover | null | undefined {
  if (!hover) return hover;
  const trustedContents = hover.contents.map(content => {
    if (content instanceof vscode.MarkdownString) {
      content.isTrusted = { enabledCommands: ['lk.openLocation'] };
      return content;
    }
    if (typeof content === 'string') {
      const markdown = new vscode.MarkdownString(content, true);
      markdown.isTrusted = { enabledCommands: ['lk.openLocation'] };
      return markdown;
    }
    return content;
  });
  return new vscode.Hover(trustedContents, hover.range);
}

async function openLkLocation(target: any) {
  if (!target?.uri || !target?.range?.start) {
    vscode.window.showErrorMessage('Invalid LK location target.');
    return;
  }
  const uri = vscode.Uri.parse(String(target.uri));
  const start = target.range.start;
  const end = target.range.end ?? start;
  const selection = new vscode.Range(
    new vscode.Position(Number(start.line) || 0, Number(start.character) || 0),
    new vscode.Position(Number(end.line) || Number(start.line) || 0, Number(end.character) || Number(start.character) || 0)
  );
  await vscode.window.showTextDocument(uri, { selection });
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
  inlayHintsShowParameters: true,
  inlayHintsShowTypes: true,
  checkingDelayMs: 120,
};

export function activate(context: vscode.ExtensionContext) {
  outputChannel = vscode.window.createOutputChannel('LK Language Server');
  context.subscriptions.push(outputChannel);
  log('LK extension is now active');

  // Create status bar item
  statusBarItem = vscode.window.createStatusBarItem(vscode.StatusBarAlignment.Right, 100);
  statusBarItem.text = '$(sync~spin) LK LSP: Starting...';
  statusBarItem.tooltip = 'LK Language Server is starting';
  statusBarItem.command = 'lk.showStatusBarMenu';
  statusBarItem.show();
  context.subscriptions.push(statusBarItem);

  // Register commands
  const startCommand = vscode.commands.registerCommand('lk.startServer', async () => {
    if (!client) {
      vscode.window.showErrorMessage('LK Language Server client not initialized');
      return;
    }
    try {
      updateStatusBar('starting');
      log('Starting LK Language Server from command');
      await withTiming('command.startServer', () => client.start());
      log('LK Language Server started from command');
      vscode.window.showInformationMessage('LK Language Server started');
    } catch (e: any) {
      logError('Failed to start LK Language Server from command', e);
      vscode.window.showErrorMessage('Failed to start LK Language Server: ' + (e?.message || e));
    }
  });

  const restartCommand = vscode.commands.registerCommand('lk.restartServer', async () => {
    if (client) {
      log('Restarting LK Language Server');
      updateStatusBar('starting');
      await client.stop();
      await withTiming('command.restartServer.start', () => client.start());
      log('LK Language Server restarted');
      vscode.window.showInformationMessage('LK Language Server restarted');
    }
  });

  const statusBarMenuCommand = vscode.commands.registerCommand('lk.showStatusBarMenu', async () => {
    const items: vscode.QuickPickItem[] = [];
    
    if (isManuallyDisabled) {
      items.push({
        label: '$(play) Enable LK LSP',
        description: 'Start the language server',
        detail: 'Enable LK Language Server'
      });
    } else {
      items.push({
        label: '$(sync) Restart LK LSP',
        description: 'Restart the language server',
        detail: 'Restart LK Language Server'
      });
      
      items.push({
        label: '$(circle-slash) Disable LK LSP',
        description: 'Temporarily disable (memory state)',
        detail: 'Disable LK Language Server temporarily'
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
      placeHolder: 'LK Language Server Actions',
      title: 'LK Language Server'
    });
    
    if (!selected) return;
    
    if (selected.label.includes('Enable')) {
      isManuallyDisabled = false;
      await vscode.commands.executeCommand('lk.startServer');
    } else if (selected.label.includes('Restart')) {
      await vscode.commands.executeCommand('lk.restartServer');
    } else if (selected.label.includes('Disable')) {
      isManuallyDisabled = true;
      if (client) {
        await client.stop();
      }
      updateStatusBar('disabled');
      vscode.window.showInformationMessage('LK Language Server disabled temporarily');
    } else if (selected.label.includes('Toggle Inlay Hints')) {
      runtime.inlayHintsEnabled = !runtime.inlayHintsEnabled;
      await vscode.workspace.getConfiguration('lk.lsp').update('inlayHints.enabled', runtime.inlayHintsEnabled, vscode.ConfigurationTarget.Workspace);
      vscode.window.showInformationMessage(`LK Inlay Hints ${runtime.inlayHintsEnabled ? 'enabled' : 'disabled'}`);
      // Trigger refresh
      await vscode.commands.executeCommand('editor.action.inlineHints.refresh');
    } else if (selected.label.includes('Parameter Hints')) {
      runtime.inlayHintsShowParameters = !runtime.inlayHintsShowParameters;
      await vscode.workspace.getConfiguration('lk.lsp').update('inlayHints.parameters.enabled', runtime.inlayHintsShowParameters, vscode.ConfigurationTarget.Workspace);
      vscode.window.showInformationMessage(`LK Parameter Hints ${runtime.inlayHintsShowParameters ? 'enabled' : 'disabled'}`);
      await vscode.commands.executeCommand('editor.action.inlineHints.refresh');
    } else if (selected.label.includes('Type Hints')) {
      runtime.inlayHintsShowTypes = !runtime.inlayHintsShowTypes;
      await vscode.workspace.getConfiguration('lk.lsp').update('inlayHints.types.enabled', runtime.inlayHintsShowTypes, vscode.ConfigurationTarget.Workspace);
      vscode.window.showInformationMessage(`LK Type Hints ${runtime.inlayHintsShowTypes ? 'enabled' : 'disabled'}`);
      await vscode.commands.executeCommand('editor.action.inlineHints.refresh');
    }
  });

  context.subscriptions.push(startCommand, restartCommand, statusBarMenuCommand);

  // Analyze current file via lk-lsp --analyze (uses relative, sanitized path)
  const analyzeCommand = vscode.commands.registerCommand('lk.analyzeCurrentFile', async () => {
    const editor = vscode.window.activeTextEditor;
    if (!editor || editor.document.languageId !== 'lk') {
      vscode.window.showWarningMessage('Open a LK file to analyze.');
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
    ], { title: 'LK Analyze Current File' });
    if (!pick) return;

    const serverPath = getServerPath();
    if (!serverPath) {
      vscode.window.showErrorMessage('LK LSP server binary not found. Build the project or configure lk.lsp.serverPath.');
      return;
    }

    const args = ['--analyze'];
    if (pick.label.startsWith('Errors')) args.push('--errors-only');
    args.push(rel);

    const out = vscode.window.createOutputChannel('LK Analysis');
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

  const openLocationCommand = vscode.commands.registerCommand('lk.openLocation', openLkLocation);
  context.subscriptions.push(openLocationCommand);

  // Check if LSP is enabled
  const config = vscode.workspace.getConfiguration('lk.lsp');
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
  runtime.inlayHintsShowParameters = config.get<boolean>('inlayHints.parameters.enabled', true);
  runtime.inlayHintsShowTypes = config.get<boolean>('inlayHints.types.enabled', true);
  runtime.checkingDelayMs = Math.max(0, Number(config.get<number>('ui.checkingDelayMs', 120)) || 120);
  
  if (!lspEnabled || isManuallyDisabled) {
    log('LK LSP is disabled in configuration or manually disabled');
    updateStatusBar('disabled');
    return;
  }

  // Get the path to the LK LSP server
  const customServerPath = config.get<string>('serverPath', '');
  const serverPath = withTiming('resolveServerPath', () => (customServerPath ? expandHome(customServerPath) : getServerPath()));

  log('Looking for LK LSP server');
  log(`Server path resolved to: ${serverPath ?? 'PATH: lk-lsp'}`);

  // If the server path is not found, show an error and return
  if (!serverPath) {
    updateStatusBar('error', 'Server not found');
    vscode.window.showErrorMessage(
      'LK LSP server not found. Please build the LK project first or configure a custom server path.'
    );
    return;
  }

  const serverOptions: ServerOptions = {
    command: serverPath,
    transport: TransportKind.stdio
  };

  const traceLevel = config.get<string>('trace', 'off');
  const isVerbose = traceLevel === 'verbose';
  const semanticTokensEnabled = runtime.semanticTokensEnabled;
  const throttleMs = runtime.semanticTokensThrottleMs;

  // Lightweight, per-document throttle map (separate for tokens and hints)
  const lastTokenReqAt = new Map<string, number>();
  // Simple per-document concurrency gates to avoid piling up work while scrolling
  const tokenInFlight = new Set<string>();
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
    provideHover(document, position, token, next) {
      const result = next(document, position, token);
      if (result && typeof (result as any).then === 'function') {
        return (result as Promise<vscode.Hover | null | undefined>).then(trustHoverCommands);
      }
      return trustHoverCommands(result as vscode.Hover | null | undefined) as any;
    },
    provideDocumentSemanticTokens(document, token, next) {
      if (!settings.semanticTokensEnabled) {
        if (isVerbose) log('Semantic tokens disabled (full)');
        return null;
      }
      // semantic tokens mode logic
      const mode = runtime.semanticTokensMode;
      if (mode === 'rangeOnly' || (mode === 'auto' && document.lineCount >= runtime.autoRangeAtLines)) {
        if (isVerbose) log('Skip full tokens due to mode');
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
          if (isVerbose) log('Semantic tokens full throttled');
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
        if (isVerbose) log('Semantic tokens disabled (range)');
        return null;
      }
      // semantic tokens mode logic
      const mode = runtime.semanticTokensMode;
      if (mode === 'fullOnly' || (mode === 'auto' && document.lineCount < runtime.autoRangeAtLines)) {
        if (isVerbose) log('Skip range tokens due to mode');
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
          if (isVerbose) log('Semantic tokens range throttled');
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
    // Inlay hints: show checking spinner, dedupe identical requests, and filter by settings.
    provideInlayHints(document, range, token, next) {
      if (!runtime.inlayHintsEnabled) {
        return null;
      }
      if (token?.isCancellationRequested) return null;
      const reqVersion = document.version;
      const key = document.uri.toString();
      const rKey = `${range.start.line}:${range.start.character}-${range.end.line}:${range.end.character}`;
      const settingsKey = `${Number(runtime.inlayHintsShowParameters)}:${Number(runtime.inlayHintsShowTypes)}`;
      const cacheKey = `${key}#v${reqVersion}#I#${rKey}#${settingsKey}`;
      if (enableCaching) {
        const cached = inlayCache.get(cacheKey);
        if (cached !== undefined) {
          return cached as any;
        }
      }
      if (dedupeInlay.has(cacheKey)) {
        return dedupeInlay.get(cacheKey)! as any;
      }
      beginChecking('inlayHints');
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
          .finally(() => { dedupeInlay.delete(cacheKey); endChecking(); });
        dedupeInlay.set(cacheKey, p as any);
        return p as any;
      } else {
        try {
          const filtered = filter(res as any);
          if (enableCaching) inlayCache.set(cacheKey, filtered as any);
          return filtered;
        } finally {
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
    if (!e.affectsConfiguration('lk.lsp')) return;
    const cfg = vscode.workspace.getConfiguration('lk.lsp');
    runtime.semanticTokensEnabled = cfg.get<boolean>('semanticTokens.enabled', true);
    runtime.semanticTokensThrottleMs = Math.max(0, Number(cfg.get<number>('semanticTokens.throttleMs', 120)) || 0);
    runtime.semanticTokensMode = (cfg.get<string>('semanticTokens.mode', 'auto') as any) || 'auto';
    runtime.autoRangeAtLines = Math.max(1, Number(cfg.get<number>('semanticTokens.autoRangeAtLines', 800)) || 800);
    runtime.inlayHintsEnabled = cfg.get<boolean>('inlayHints.enabled', true);
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
    documentSelector: [
      { scheme: 'file', language: 'lk' },
      { scheme: 'untitled', language: 'lk' },
    ],
    synchronize: {
      configurationSection: 'lk'
    },
    // Initialize options for semantic highlighting
    initializationOptions: {
      // Enable semantic highlighting
      semanticHighlighting: runtime.semanticTokensEnabled,
      // Custom configuration for LK
      lk: {
        enableSemanticTokens: runtime.semanticTokensEnabled
      }
    },
    // Never auto-reveal the output unless user explicitly opens it
    revealOutputChannelOn: RevealOutputChannelOn.Never,
    outputChannel,
    traceOutputChannel: traceLevel !== 'off' ? outputChannel : undefined,
    middleware
  };

  client = withTiming('createLanguageClient', () => new LanguageClient(
    'lk',
    'LK Language Server',
    serverOptions,
    clientOptions
  ));

  if (isVerbose) {
    log(`Starting LK Language Server: ${serverPath}`);
  }
  
  // Add error handling for the client itself
  client.onDidChangeState((event) => {
    if (isVerbose) {
      log(`LSP client state change: ${event.oldState} -> ${event.newState}`);
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
  let startTimeout: NodeJS.Timeout | undefined;
  const timeoutPromise = new Promise((_, reject) => {
    startTimeout = setTimeout(() => reject(new Error('LSP server start timeout after 10 seconds')), 10000);
  });
  
  Promise.race([startPromise, timeoutPromise])
    .then(() => {
      if (startTimeout) {
        clearTimeout(startTimeout);
        startTimeout = undefined;
      }
      if (isVerbose && autoStart) {
        log('LK Language Server started successfully');
        // Check if semantic highlighting is enabled
        const editorConfig = vscode.workspace.getConfiguration('editor');
        const semanticHighlighting = editorConfig.get('semanticHighlighting.enabled');
        log(`Semantic highlighting enabled: ${semanticHighlighting}`);
      }
      updateStatusBar('running');
    })
    .catch((error) => {
      if (startTimeout) {
        clearTimeout(startTimeout);
        startTimeout = undefined;
      }
      logError('Failed to start LK Language Server', error);
      vscode.window.showErrorMessage('Failed to start LK Language Server: ' + error.message);
      updateStatusBar('error', 'Start failed');
      
      // Try to stop the client if it's in a bad state
      if (client) {
        client.stop().catch(stopError => {
          logError('Error stopping client after failure', stopError);
        });
      }
    });
  
  // Mark as checking when LK documents change; diagnostics will clear it.
  // Save alone does not necessarily produce a server request, so it must not
  // create a long-lived checking state.
  context.subscriptions.push(vscode.workspace.onDidChangeTextDocument(e => {
    if (e.document.languageId === 'lk') {
      nudgeChecking();
    }
  }));
  // React to configuration changes at runtime
  context.subscriptions.push(vscode.workspace.onDidChangeConfiguration(e => {
    if (
      e.affectsConfiguration('lk.lsp.semanticTokens.enabled') ||
      e.affectsConfiguration('lk.lsp.semanticTokens.throttleMs')
    ) {
      const cfg = vscode.workspace.getConfiguration('lk.lsp');
      settings.semanticTokensEnabled = cfg.get<boolean>('semanticTokens.enabled', true);
      settings.throttleMs = Math.max(0, Number(cfg.get<number>('semanticTokens.throttleMs', 40)) || 0);
      if (isVerbose) log(`Updated semantic tokens settings ${JSON.stringify(settings)}`);
    }
  }));
}

function getServerPath(): string | undefined {
  // Try to find the lk-lsp executable in different locations
  const exe = process.platform === 'win32' ? '.exe' : '';
  const possiblePaths = [
    // When developing inside repo: extension is at ecosystem/vsc-ext/lsp/out
    // Look for cargo build outputs at repo root target/{debug,release}
    path.join(__dirname, '..', '..', '..', '..', 'target', 'debug', `lk-lsp${exe}`),
    path.join(__dirname, '..', '..', '..', '..', 'target', 'release', `lk-lsp${exe}`),
    // Fallbacks that may exist depending on packaging layout
    path.join(__dirname, '..', '..', '..', 'target', 'debug', `lk-lsp${exe}`),
    path.join(__dirname, '..', '..', '..', 'target', 'release', `lk-lsp${exe}`),
    path.join(__dirname, '..', '..', 'target', 'debug', `lk-lsp${exe}`),
    path.join(__dirname, '..', '..', 'target', 'release', `lk-lsp${exe}`),
    // Common user install locations
    expandHome(`~/.cargo/bin/lk-lsp${exe}`),
    // Homebrew on macOS
    '/opt/homebrew/bin/lk-lsp',
    '/usr/local/bin/lk-lsp',
  ];

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

  // Fall back to PATH resolution by returning command name.
  // This allows the system to find lk-lsp if it's on the PATH.
  // The vscode-languageclient will use child_process.spawn which
  // resolves from PATH automatically.
  return process.platform === 'win32' ? 'lk-lsp.exe' : 'lk-lsp';
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
      statusBarItem.text = '$(sync~spin) LK LSP: Starting...';
      statusBarItem.tooltip = 'LK Language Server is starting';
      break;
    case 'checking':
      statusBarItem.text = '$(sync~spin) LK LSP: Checking...';
      statusBarItem.tooltip = 'LK Language Server is analyzing/validating';
      break;
    case 'running':
      statusBarItem.text = '$(check) LK LSP: Running';
      statusBarItem.tooltip = 'LK Language Server is running';
      break;
    case 'stopped':
      statusBarItem.text = '$(circle-slash) LK LSP: Stopped';
      statusBarItem.tooltip = 'LK Language Server is stopped';
      break;
    case 'error':
      statusBarItem.text = '$(error) LK LSP: Error';
      statusBarItem.tooltip = customMessage ? `LK Language Server error: ${customMessage}` : 'LK Language Server error';
      break;
    case 'disabled':
      statusBarItem.text = '$(circle-slash) LK LSP: Disabled';
      statusBarItem.tooltip = isManuallyDisabled ? 'LK Language Server is temporarily disabled (click to enable)' : 'LK Language Server is disabled in settings';
      break;
    default:
      statusBarItem.text = '$(question) LK LSP: Unknown';
      statusBarItem.tooltip = 'LK Language Server status unknown';
  }
}

function beginChecking(_reason?: string) {
  if (!statusBarItem || isManuallyDisabled) {
    return;
  }
  const wasIdle = checkInFlight === 0;
  checkInFlight++;
  if (wasIdle) {
    if (nudgeTimer) {
      clearTimeout(nudgeTimer);
      nudgeTimer = undefined;
    }
    if (nudgeClearTimer) {
      clearTimeout(nudgeClearTimer);
      nudgeClearTimer = undefined;
    }
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
  if (nudgeTimer) {
    clearTimeout(nudgeTimer);
    nudgeTimer = undefined;
  }
  if (nudgeClearTimer) {
    clearTimeout(nudgeClearTimer);
    nudgeClearTimer = undefined;
  }
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
  if (checkInFlight !== 0) return;
  if (checkIdleTimer) {
    clearTimeout(checkIdleTimer);
    checkIdleTimer = undefined;
  }
  if (nudgeTimer) clearTimeout(nudgeTimer);
  if (nudgeClearTimer) {
    clearTimeout(nudgeClearTimer);
    nudgeClearTimer = undefined;
  }
  const delay = Math.max(0, runtime.checkingDelayMs || 0);
  nudgeTimer = setTimeout(() => {
    nudgeTimer = undefined;
    if (checkInFlight === 0 && !isManuallyDisabled) {
      updateStatusBar('checking');
      nudgeClearTimer = setTimeout(() => {
        nudgeClearTimer = undefined;
        if (checkInFlight === 0 && !isManuallyDisabled) {
          updateStatusBar('running');
        }
      }, 1500);
    }
  }, delay);
}

export function deactivate(): Thenable<void> | undefined {
  if (nudgeTimer) {
    clearTimeout(nudgeTimer);
    nudgeTimer = undefined;
  }
  if (nudgeClearTimer) {
    clearTimeout(nudgeClearTimer);
    nudgeClearTimer = undefined;
  }
  if (!client) {
    return undefined;
  }
  updateStatusBar('stopped');
  return client.stop();
}
