import { SchemaCache, ToolSchema } from './schema-cache.js';
import { log } from './logger.js';

// ─── Types ───────────────────────────────────────────────────────────

export interface ToolSearchResult {
  server: string;
  name: string;
  description: string;
  inputSchema: Record<string, unknown>;
  score: number;
}

interface IndexEntry {
  server: string;
  tool: ToolSchema;
  tokens: string[];
}

// ─── Tool Search Engine ──────────────────────────────────────────────

export class ToolSearchEngine {
  private index: IndexEntry[] = [];
  private serverSummaries: Map<string, string> = new Map();

  constructor(private schemaCache: SchemaCache) {}

  /** Build search index from cached schemas */
  buildIndex(): void {
    this.index = [];
    this.serverSummaries.clear();

    const allTools = this.schemaCache.getAllTools(false);

    for (const tool of allTools) {
      const server = this.schemaCache.getServerForTool(tool.name);
      if (!server) continue;

      // Tokenize name + description for matching
      const tokens = this.tokenize(
        `${tool.name} ${tool.description ?? ''} ${server}`
      );

      this.index.push({ server, tool, tokens });

      // Build per-server summary
      if (!this.serverSummaries.has(server)) {
        this.serverSummaries.set(server, '');
      }
    }

    // Generate compact server summaries
    this.buildServerSummaries();

    log.info(`Tool search index built: ${this.index.length} tools indexed`);
  }

  /** Search tools by query, return top N matches */
  search(query: string, maxResults: number = 10): ToolSearchResult[] {
    const queryTokens = this.tokenize(query);

    if (queryTokens.length === 0) {
      return this.index.slice(0, maxResults).map(e => this.toResult(e, 0));
    }

    const scored: { entry: IndexEntry; score: number }[] = [];

    for (const entry of this.index) {
      const score = this.calculateScore(queryTokens, entry);
      if (score > 0) {
        scored.push({ entry, score });
      }
    }

    scored.sort((a, b) => b.score - a.score);

    return scored
      .slice(0, maxResults)
      .map(s => this.toResult(s.entry, s.score));
  }

  /** Generate the compact catalog for the meta-tool description */
  getCatalog(): string {
    const lines: string[] = [];

    for (const serverName of this.schemaCache.serverNames) {
      const summary = this.serverSummaries.get(serverName) || '';
      lines.push(`[${serverName}] ${summary}`);
    }

    return lines.join('\n');
  }

  /** Get total indexed tool count */
  get toolCount(): number {
    return this.index.length;
  }

  // ─── Private ─────────────────────────────────────────────────────

  private buildServerSummaries(): void {
    const serverTools = new Map<string, ToolSchema[]>();

    for (const entry of this.index) {
      const list = serverTools.get(entry.server) || [];
      list.push(entry.tool);
      serverTools.set(entry.server, list);
    }

    for (const [server, tools] of serverTools) {
      // Extract key capability keywords from tool names
      const keywords = new Set<string>();
      const toolNames: string[] = [];

      for (const tool of tools) {
        toolNames.push(tool.name);
        // Extract meaningful words from tool name
        const words = tool.name
          .replace(/[_-]/g, ' ')
          .split(/\s+/)
          .filter(w => w.length > 2 && !STOP_WORDS.has(w.toLowerCase()));
        words.forEach(w => keywords.add(w.toLowerCase()));
      }

      // Compact summary: server capabilities + tool count
      const topKeywords = [...keywords].slice(0, 8).join(', ');
      this.serverSummaries.set(
        server,
        `(${tools.length} tools) ${topKeywords}`
      );
    }
  }

  private calculateScore(queryTokens: string[], entry: IndexEntry): number {
    let score = 0;
    const toolNameLower = entry.tool.name.toLowerCase();
    const descLower = (entry.tool.description ?? '').toLowerCase();
    const serverLower = entry.server.toLowerCase();

    for (const qt of queryTokens) {
      // Exact tool name match (highest weight)
      if (toolNameLower === qt) {
        score += 100;
      }
      // Tool name contains query token
      else if (toolNameLower.includes(qt)) {
        score += 30;
      }

      // Server name match
      if (serverLower.includes(qt)) {
        score += 15;
      }

      // Description match
      if (descLower.includes(qt)) {
        score += 10;
      }

      // Token-level matching (partial/fuzzy)
      for (const token of entry.tokens) {
        if (token === qt) {
          score += 5;
        } else if (token.startsWith(qt) || qt.startsWith(token)) {
          score += 2;
        }
      }
    }

    return score;
  }

  private tokenize(text: string): string[] {
    return text
      .toLowerCase()
      .replace(/[^a-z0-9\s]/g, ' ')
      .split(/\s+/)
      .filter(t => t.length > 1 && !STOP_WORDS.has(t));
  }

  private toResult(entry: IndexEntry, score: number): ToolSearchResult {
    return {
      server: entry.server,
      name: entry.tool.name,
      description: entry.tool.description ?? '',
      inputSchema: entry.tool.inputSchema,
      score,
    };
  }
}

// ─── Stop words to ignore during tokenization ────────────────────────

const STOP_WORDS = new Set([
  'the', 'a', 'an', 'is', 'are', 'was', 'were', 'be', 'been', 'being',
  'have', 'has', 'had', 'do', 'does', 'did', 'will', 'would', 'could',
  'should', 'may', 'might', 'can', 'shall', 'to', 'of', 'in', 'for',
  'on', 'with', 'at', 'by', 'from', 'as', 'into', 'about', 'between',
  'through', 'and', 'but', 'or', 'not', 'no', 'if', 'then', 'than',
  'so', 'up', 'out', 'it', 'its', 'this', 'that', 'all', 'any',
]);
