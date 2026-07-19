// Token redaction utility — R6 of Cipher's wiring contract (aperture-ttzz).
//
// Every log line, audit body, error message, or stderr emission from this
// MCP server MUST pass through redact() before leaving the process. The
// upstream Sentry MCP token is injected via env (SENTRY_ACCESS_TOKEN) at
// startup; if it ever appears in a captured log line — verbatim, as a
// "Bearer ..." header, or even as a leading prefix — the swarm has
// effectively published the workspace token to Loki.
//
// The unit test (tests/redact.test.ts) injects a known synthetic token,
// runs every gate path, and greps captured logs for:
//   - The literal "Bearer " (case-insensitive)
//   - Any 8-character prefix of the injected token
//   - Authorization header value in any form
//
// If a future code path emits a log line without redact(), the test fails
// and CI blocks merge.

export interface Redactor {
  redact(text: string): string;
  redactObject<T>(obj: T): T;
}

const BEARER_PATTERN = /Bearer\s+[A-Za-z0-9_.\-]+/gi;
// Authorization-header pattern: catches non-Bearer auth schemes (Basic,
// raw tokens) but skips "Bearer ..." values so the BEARER_PATTERN above
// can preserve the "Bearer [REDACTED]" shape downstream readers expect.
// Also skips already-redacted values so the regex doesn't double-strip.
const AUTHORIZATION_HEADER_PATTERN =
  /(authorization\s*[:=]\s*)(?!Bearer\b)(?!\[REDACTED\])["']?[^"'\s,}]+/gi;

/**
 * Create a redactor bound to a specific live token. The token's substring is
 * redacted plus any 8+ character prefix appearing in unrelated form. The
 * Bearer-token regex catches arbitrary OAuth bearers regardless of which
 * token is the live one (defense in depth — third-party libs sometimes log
 * their own headers).
 */
export function createRedactor(liveToken: string | null): Redactor {
  // We allow null so that startup-time error paths (allowlist missing,
  // token missing) can still be redacted-safe with no token configured.
  const tokenPrefixes: string[] = [];
  if (liveToken && liveToken.length >= 8) {
    tokenPrefixes.push(liveToken);
    // The first 8 chars are the most common "leaked prefix" form
    // (debug output that truncates a token to its head).
    tokenPrefixes.push(liveToken.slice(0, 8));
  }

  function redactString(input: string): string {
    if (typeof input !== "string" || input.length === 0) {
      return input;
    }
    let out = input;
    // Bearer headers and Authorization headers first — these always-redact
    // regardless of token match.
    out = out.replace(BEARER_PATTERN, "Bearer [REDACTED]");
    out = out.replace(AUTHORIZATION_HEADER_PATTERN, "$1[REDACTED]");
    // Then strip every known token prefix substring.
    for (const tok of tokenPrefixes) {
      if (tok.length === 0) continue;
      // Global replace — split/join is cheap and avoids regex-escape issues
      // when tokens contain regex metacharacters.
      while (out.includes(tok)) {
        out = out.split(tok).join("[REDACTED]");
      }
    }
    return out;
  }

  function redactObject<T>(obj: T): T {
    // Stringify-then-parse is the cheapest deep redaction strategy and
    // matches what hits Loki anyway. Non-stringifiable values (functions,
    // symbols, circular refs) survive by structural deep copy fallback.
    try {
      const s = JSON.stringify(obj);
      if (s === undefined) return obj;
      return JSON.parse(redactString(s)) as T;
    } catch {
      return obj;
    }
  }

  return {
    redact: redactString,
    redactObject,
  };
}

/**
 * Convenience global redactor — initialised once at server boot in index.ts
 * via setActiveRedactor(). Every module that emits a log line imports
 * `redact` and uses it without holding a redactor reference.
 *
 * If setActiveRedactor() is never called (test contexts, misconfigured
 * boot), redact() falls back to a no-token redactor that still strips
 * Bearer-style patterns. Defense in depth — never silently no-op.
 */
let activeRedactor: Redactor = createRedactor(null);

export function setActiveRedactor(r: Redactor): void {
  activeRedactor = r;
}

export function redact(text: string): string {
  return activeRedactor.redact(text);
}

export function redactObject<T>(obj: T): T {
  return activeRedactor.redactObject(obj);
}
