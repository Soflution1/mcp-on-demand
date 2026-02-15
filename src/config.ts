import { readFileSync, existsSync } from 'fs';
import { resolve } from 'path';
import { homedir } from 'os';

// ─── Types ───────────────────────────────────────────────────────────

export interface ServerConfig {
  command: string;
  args?: string[];
  env?: Record<string, string>;
  group?: string;
  idleTimeout?: number;
  persistent?: boolean;
}

export interface ProxySettings {
  idleTimeout: number;
  cacheDir: string;
  logLevel: 'debug' | 'info' | 'warn' | 'error' | 'silent';
  startupTimeout: number;
  prefixTools: boolean;
  mode: 'passthrough' | 'tool-search';
}

export interface ProxyConfig {
  settings: ProxySettings;
  servers: Record<string, ServerConfig>;
  skipped?: Record<string, string>;
}

// ─── Defaults ────────────────────────────────────────────────────────

const DEFAULT_SETTINGS: ProxySettings = {
  idleTimeout: 300,
  cacheDir: resolve(homedir(), '.mcp-on-demand', 'cache'),
  logLevel: 'info',
  startupTimeout: 30000,
  prefixTools: false,
  mode: 'tool-search',
};

// ─── Helpers ─────────────────────────────────────────────────────────

export function expandPath(p: string): string {
  if (p.startsWith('~')) return resolve(homedir(), p.slice(2));
  return resolve(p);
}

// ─── Cursor config auto-detection ────────────────────────────────────

function getCursorConfigPaths(): string[] {
  const home = homedir();
  return [
    resolve(home, '.cursor', 'mcp.json'),
    resolve(home, 'Library', 'Application Support', 'Cursor', 'mcp.json'),
    resolve(home, '.config', 'cursor', 'mcp.json'),
    resolve(home, 'AppData', 'Roaming', 'Cursor', 'mcp.json'),
  ];
}

function findCursorConfig(): string | null {
  for (const p of getCursorConfigPaths()) {
    if (existsSync(p)) return p;
  }
  return null;
}

const SELF_IDENTIFIERS = ['mcp-on-demand', '@soflution/mcp-on-demand'];

function isSelf(name: string, cfg: any): boolean {
  const nameLower = name.toLowerCase().replace(/[^a-z0-9]/g, '');
  if (nameLower.includes('mcpondemand')) return true;
  if (cfg.args) {
    const argsStr = cfg.args.join(' ').toLowerCase();
    if (SELF_IDENTIFIERS.some(id => argsStr.includes(id))) return true;
  }
  return false;
}

/**
 * Auto-detect servers from Cursor mcp.json.
 * Filters out: this proxy, URL-based servers, disabled servers.
 */
export function autoDetectConfig(): ProxyConfig {
  const cursorPath = findCursorConfig();

  if (!cursorPath) {
    throw new Error(
      'Could not find Cursor MCP config. Looked in:\n' +
      getCursorConfigPaths().map(p => `  - ${p}`).join('\n')
    );
  }

  const raw = JSON.parse(readFileSync(cursorPath, 'utf-8'));
  const allServers = raw.mcpServers || {};

  const servers: Record<string, ServerConfig> = {};
  const skipped: Record<string, string> = {};

  for (const [name, cfg] of Object.entries(allServers) as [string, any][]) {
    if (isSelf(name, cfg)) {
      skipped[name] = 'self (this proxy)';
      continue;
    }
    if (cfg.disabled === true) {
      skipped[name] = 'disabled';
      continue;
    }
    if (cfg.url && !cfg.command) {
      skipped[name] = 'url-based (kept in Cursor config)';
      continue;
    }
    if (!cfg.command) {
      skipped[name] = 'no command defined';
      continue;
    }

    servers[name] = {
      command: cfg.command,
      args: cfg.args,
      env: cfg.env,
    };
  }

  return {
    settings: { ...DEFAULT_SETTINGS },
    servers,
    skipped,
  };
}

// ─── Manual config loader ────────────────────────────────────────────

export function loadConfig(configPath: string): ProxyConfig {
  const fullPath = expandPath(configPath);
  if (!existsSync(fullPath)) {
    throw new Error(`Config file not found: ${fullPath}`);
  }

  const raw = JSON.parse(readFileSync(fullPath, 'utf-8'));
  const settings: ProxySettings = { ...DEFAULT_SETTINGS, ...raw.settings };
  settings.cacheDir = expandPath(settings.cacheDir);

  return { settings, servers: raw.servers };
}
