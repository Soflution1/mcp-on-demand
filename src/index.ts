import { autoDetectConfig, loadConfig, expandPath } from './config.js';
import { ProxyServer } from './proxy-server.js';
import { setLogLevel, log } from './logger.js';
import { homedir } from 'os';
import { resolve } from 'path';
import { existsSync } from 'fs';

const VERSION = '1.2.0';
const DEFAULT_CONFIG = resolve(homedir(), '.mcp-on-demand', 'config.json');

async function main() {
  const args = process.argv.slice(2);

  // Parse optional flags
  let manualConfigPath: string | null = null;
  const configIdx = args.indexOf('--config');
  if (configIdx >= 0 && args[configIdx + 1]) {
    manualConfigPath = expandPath(args[configIdx + 1]);
  }

  const logIdx = args.indexOf('--log-level');
  if (logIdx >= 0 && args[logIdx + 1]) {
    setLogLevel(args[logIdx + 1] as any);
  }

  // Parse --mode flag (tool-search | passthrough)
  let modeOverride: 'tool-search' | 'passthrough' | null = null;
  const modeIdx = args.indexOf('--mode');
  if (modeIdx >= 0 && args[modeIdx + 1]) {
    const m = args[modeIdx + 1];
    if (m === 'tool-search' || m === 'passthrough') {
      modeOverride = m;
    } else {
      log.warn(`Invalid mode "${m}". Using default. Valid: tool-search, passthrough`);
    }
  }

  try {
    let config;

    if (manualConfigPath && existsSync(manualConfigPath)) {
      config = loadConfig(manualConfigPath);
      log.info(`Using manual config: ${manualConfigPath}`);
    } else if (existsSync(DEFAULT_CONFIG)) {
      config = loadConfig(DEFAULT_CONFIG);
      log.info(`Using existing config: ${DEFAULT_CONFIG}`);
    } else {
      log.info('First run detected. Auto-reading Cursor MCP config...');
      config = autoDetectConfig();

      const serverCount = Object.keys(config.servers).length;
      const skippedCount = Object.keys(config.skipped || {}).length;

      log.info(`Found ${serverCount} stdio servers to proxy`);

      if (config.skipped && skippedCount > 0) {
        for (const [name, reason] of Object.entries(config.skipped)) {
          log.info(`  Skipped: ${name} (${reason})`);
        }
      }

      if (serverCount === 0) {
        log.error('No servers found to proxy. Check your Cursor mcp.json.');
        process.exit(1);
      }
    }

    // Apply mode override from CLI flag
    if (modeOverride) {
      config.settings.mode = modeOverride;
    }

    setLogLevel(config.settings.logLevel);
    log.info(`mcp-on-demand v${VERSION}`);
    log.info(`Mode: ${config.settings.mode}`);
    log.info(`Servers: ${Object.keys(config.servers).length}`);
    log.info(`Idle timeout: ${config.settings.idleTimeout}s`);

    const proxy = new ProxyServer(config);

    const shutdown = async () => {
      await proxy.shutdown();
      process.exit(0);
    };

    process.on('SIGINT', shutdown);
    process.on('SIGTERM', shutdown);

    await proxy.start();
  } catch (err) {
    log.error('Fatal:', err);
    process.exit(1);
  }
}

main();
